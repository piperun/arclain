//! Vault and preferences operations

use super::AppState;
use anyhow::Result;
use arclain_core::services::SecretsService;
use arclain_core::utilities::PassRule;
use arclain_db::{open_databases, set_config, ConfigDb, DbPaths, SecretsKey};
use arclain_plugins::PluginManager;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::warn;

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

        // Always persist overrides into plain config.sqlite
        let cfg_conn = ConfigDb::open(&paths.config_db)?.into_sqlite_db();
        if let Some(ref dbp) = secrets_db_path {
            let _ = cfg_conn.with_connection(|conn| set_config(conn, "secrets_db_path", dbp));
        }
        if let Some(ref kfp) = key_file_path {
            let _ = cfg_conn.with_connection(|conn| set_config(conn, "key_file_path", kfp));
        }
        if let Some(ref pol) = encrypted_crc_policy {
            let _ = cfg_conn.with_connection(|conn| set_config(conn, "encrypted_crc_policy", pol));
            self.encrypted_crc_policy = pol.clone();
        }

        // Try to open encrypted secrets DB only if we have a key file
        if let Some(kp) = paths.key_file.clone() {
            let key = SecretsKey::load_from_file(&kp)?;
            let dbs = open_databases(&paths, &key)?;
            self.db_paths = Some(paths.clone());

            // Update plugin manager with new cache
            if let Some(manager_arc) = plugin_manager {
                let lib_svc = arclain_core::LibraryService::new(dbs.cache_pool.clone());
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
            let lib_svc = arclain_core::LibraryService::new(new_dbs.cache_pool.clone());
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
            let lib_svc = arclain_core::LibraryService::new(new_dbs.cache_pool.clone());
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
