//! Vault and preferences operations

use super::AppState;
use anyhow::Result;
use arclain_core::services::SecretsService;
use arclain_core::utilities::PassRule;
use arclain_db::{open_databases, set_config, ConfigDb, DbPaths, SecretsKey, SqliteDb};
use arclain_plugins::PluginManager;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

/// Persist preference overrides to the config DB. Aborts on the first
/// failure rather than swallowing — see audit finding H4: previously
/// each `set_config` was wrapped in `let _ =`, so an on-disk write
/// failure left in-memory and on-disk state diverged. The next launch
/// would then read the old values from disk and ignore the user's
/// changes.
pub(crate) fn persist_preferences_overrides(
    cfg_conn: &SqliteDb,
    secrets_db_path: Option<&str>,
    key_file_path: Option<&str>,
    encrypted_crc_policy: Option<&str>,
) -> Result<()> {
    if let Some(p) = secrets_db_path {
        cfg_conn.with_connection(|conn| set_config(conn, "secrets_db_path", p))?;
    }
    if let Some(p) = key_file_path {
        cfg_conn.with_connection(|conn| set_config(conn, "key_file_path", p))?;
    }
    if let Some(p) = encrypted_crc_policy {
        cfg_conn.with_connection(|conn| set_config(conn, "encrypted_crc_policy", p))?;
    }
    Ok(())
}

impl AppState {
    /// Apply Preferences changes: persist overrides and (re)open SQLCipher DBs.
    pub fn apply_preferences(
        &mut self,
        key_file_path: Option<String>,
        secrets_db_path: Option<String>,
        encrypted_crc_policy: Option<String>,
        plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
    ) -> Result<()> {
        // Start from existing paths or defaults
        let mut paths = if let Some(p) = self.db_paths.clone() {
            p
        } else {
            DbPaths::calculate_defaults("arclain")?
        };

        if let Some(ref dbp) = secrets_db_path {
            paths.secrets_db = PathBuf::from(dbp);
        }
        if let Some(ref kfp) = key_file_path {
            paths.key_file = Some(PathBuf::from(kfp));
        }

        // Always persist overrides into plain config.sqlite. The helper
        // aborts on the first write failure so we don't end up with
        // in-memory state ahead of on-disk state.
        let cfg_conn = ConfigDb::open(&paths.config_db)?.into_sqlite_db();
        persist_preferences_overrides(
            &cfg_conn,
            secrets_db_path.as_deref(),
            key_file_path.as_deref(),
            encrypted_crc_policy.as_deref(),
        )?;
        if let Some(ref pol) = encrypted_crc_policy {
            self.encrypted_crc_policy = pol.clone();
        }

        // Try to open encrypted secrets DB only if we have a key file
        if let Some(kp) = paths.key_file.clone() {
            let key = SecretsKey::load_from_file(&kp)?;
            let dbs = open_databases(&paths, &key)?;
            self.db_paths = Some(paths.clone());

            // Update plugin manager with new cache
            if let Some(manager_arc) = plugin_manager {
                let lib_svc = arclain_core::LibraryService::new(&paths.cache_db)?;
                manager_arc.lock().set_library_service(Arc::new(lib_svc));
            }

            self.dbs = Some(dbs);

            // Reload pass rules
            if let Some(ref dbs_ref) = self.dbs {
                if let Ok(rules) = dbs_ref.secrets.list_pass_rules() {
                    self.pass_rules = rules
                        .into_iter()
                        .map(|r| PassRule {
                            name: r.name,
                            pattern: r.pattern,
                            password: r.password,
                            priority: r.priority,
                            enabled: r.enabled,
                        })
                        .collect();
                }
            }
        } else {
            self.db_paths = Some(paths.clone());
        }

        Ok(())
    }

    pub fn move_vault(
        &mut self,
        dest_path: &str,
        plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
    ) -> Result<()> {
        // Close existing DBs to avoid file locks
        if let Some(_) = self.dbs.take() {
            // dropped
        }

        let (new_paths, new_dbs) = SecretsService::move_vault(&mut self.db_paths, dest_path)?;

        self.db_paths = Some(new_paths.clone());

        // Update plugin manager
        if let Some(manager_arc) = plugin_manager {
            let lib_svc = arclain_core::LibraryService::new(&new_paths.cache_db)?;
            manager_arc.lock().set_library_service(Arc::new(lib_svc));
        }

        self.dbs = Some(new_dbs);

        // Reload rules
        if let Some(ref dbs_ref) = self.dbs {
            if let Ok(rules) = dbs_ref.secrets.list_pass_rules() {
                self.pass_rules = rules
                    .into_iter()
                    .map(|r| PassRule {
                        name: r.name,
                        pattern: r.pattern,
                        password: r.password,
                        priority: r.priority,
                        enabled: r.enabled,
                    })
                    .collect();
            }
        }

        Ok(())
    }

    pub fn rekey_vault(
        &mut self,
        new_key_file_path: &str,
        plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
    ) -> Result<()> {
        // Close existing DBs
        if let Some(_) = self.dbs.take() {
            // dropped
        }

        let (new_paths, new_dbs, rules) =
            SecretsService::rekey_vault(&mut self.db_paths, new_key_file_path)?;

        self.db_paths = Some(new_paths.clone());

        // Update plugin manager
        if let Some(manager_arc) = plugin_manager {
            let lib_svc = arclain_core::LibraryService::new(&new_paths.cache_db)?;
            manager_arc.lock().set_library_service(Arc::new(lib_svc));
        }

        self.dbs = Some(new_dbs);
        self.pass_rules = rules
            .into_iter()
            .map(|r| PassRule {
                name: r.name,
                pattern: r.pattern,
                password: r.password,
                priority: r.priority,
                enabled: r.enabled,
            })
            .collect();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_db::DbConnection;
    use tempfile::TempDir;

    fn corrupt_app_config_schema(path: &std::path::Path) {
        let conn = DbConnection::open(path).expect("opening sqlite for setup");
        conn.execute(
            "CREATE TABLE app_config (key TEXT PRIMARY KEY, wrong_col TEXT)",
            [],
        )
        .expect("creating mis-schema'd app_config");
    }

    /// Regression test for H4 from `docs/AUDIT_2026-05-03.md`.
    ///
    /// Pre-fix, `apply_preferences` wrapped each `set_config` call in
    /// `let _ =`, so a failed write left in-memory state (e.g.
    /// `self.encrypted_crc_policy`) ahead of on-disk state. Next launch
    /// would read the stale on-disk values and silently undo what the
    /// user thought they had saved.
    ///
    /// `persist_preferences_overrides` (extracted as part of the fix)
    /// now propagates errors via `?`. Force the failure by pre-creating
    /// `app_config` with a wrong column schema — `ConfigDb::open`'s
    /// `CREATE TABLE IF NOT EXISTS` is a no-op once the table exists,
    /// so the corrupt schema survives, and `set_config`'s
    /// `INSERT INTO app_config(key, value)` then fails because there's
    /// no `value` column.
    #[test]
    fn h4_persist_preferences_overrides_propagates_set_config_failure() {
        let temp = TempDir::new().unwrap();
        let cfg_path = temp.path().join("config.sqlite");
        corrupt_app_config_schema(&cfg_path);

        let cfg = ConfigDb::open(&cfg_path).unwrap().into_sqlite_db();

        let result = persist_preferences_overrides(
            &cfg,
            Some("/some/secrets.redb"),
            None,
            None,
        );

        assert!(
            result.is_err(),
            "H4 fix regressed: helper returned Ok despite the config-DB write failing",
        );
    }

    /// With a healthy schema, the helper should succeed and write each
    /// provided override.
    #[test]
    fn h4_persist_preferences_overrides_writes_each_override() {
        use arclain_db::get_config;

        let temp = TempDir::new().unwrap();
        let cfg_path = temp.path().join("config.sqlite");
        let cfg = ConfigDb::open(&cfg_path).unwrap().into_sqlite_db();

        persist_preferences_overrides(
            &cfg,
            Some("/secrets.redb"),
            Some("/key.bin"),
            Some("strict"),
        )
        .expect("healthy schema should write");

        let secrets = cfg
            .with_connection(|conn| get_config(conn, "secrets_db_path"))
            .unwrap();
        let key = cfg
            .with_connection(|conn| get_config(conn, "key_file_path"))
            .unwrap();
        let pol = cfg
            .with_connection(|conn| get_config(conn, "encrypted_crc_policy"))
            .unwrap();

        assert_eq!(secrets.as_deref(), Some("/secrets.redb"));
        assert_eq!(key.as_deref(), Some("/key.bin"));
        assert_eq!(pol.as_deref(), Some("strict"));
    }
}
