//! Service for managing secrets and vault operations
//!
//! Handles complex operations like rekeying and moving the vault,
//! which involve file system operations and multiple database interactions.

use anyhow::Result;
use arclain_db::{open_databases, set_config, DbPaths, SecretsDb, SecretsKey};
use std::path::{Path, PathBuf};

pub struct SecretsService;

impl SecretsService {
    /// Move the vault to a new location
    pub fn move_vault(
        db_paths: &mut Option<DbPaths>,
        dest_path: &str,
    ) -> Result<(DbPaths, arclain_db::ConfigDbs)> {
        use arclain_db::ConfigDb;
        use std::fs;

        // Establish paths
        let mut paths = if let Some(p) = db_paths.clone() {
            p
        } else {
            DbPaths::calculate_defaults("arclain")?
        };

        let src = paths.secrets_db.clone();
        let dst = PathBuf::from(dest_path);

        // Simple file copy for redb
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&src, &dst)?;

        // Set secure permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&dst, perms)?;
        }

        // Persist new path in config DB. Abort if this fails — otherwise
        // the user has a copied vault but on-disk config still points at
        // the old one, so the next launch opens the wrong DB.
        let cfg_conn = ConfigDb::open(&paths.config_db)?.into_sqlite_db();
        cfg_conn
            .with_connection(|conn| set_config(conn, "secrets_db_path", &dst.to_string_lossy()))?;

        // Update paths and reopen if key is available
        paths.secrets_db = dst;

        // Re-open databases
        let key_path = paths
            .key_file
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No key file available for re-opening"))?;
        let key = SecretsKey::load_from_file(&key_path)?;
        let dbs = open_databases(&paths, &key)?;

        Ok((paths, dbs))
    }

    /// Rekey the vault with a new key file
    pub fn rekey_vault(
        db_paths: &mut Option<DbPaths>,
        new_key_file_path: &str,
    ) -> Result<(DbPaths, arclain_db::ConfigDbs, Vec<arclain_db::DbPassRule>)> {
        // Resolve paths
        let mut paths = if let Some(p) = db_paths.clone() {
            p
        } else {
            DbPaths::calculate_defaults("arclain")?
        };

        // Ensure current key exists
        let old_key_path = paths
            .key_file
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No current key file configured"))?;
        let old_key = SecretsKey::load_from_file(&old_key_path)?;
        let new_key = SecretsKey::load_from_file(Path::new(new_key_file_path))?;

        // 1. Read all data with old key (from existing DB on disk since we closed connection)
        let rules = {
            let old_db = SecretsDb::open(&paths.secrets_db, &old_key.as_bytes())?;
            old_db.list_pass_rules()?
        };

        // 2. Create backup
        let backup_path = paths.secrets_db.with_extension("redb.backup");
        std::fs::copy(&paths.secrets_db, &backup_path)?;

        // 3. Remove old database
        std::fs::remove_file(&paths.secrets_db)?;

        // 4. Create new one with new key
        let new_dbs = open_databases(&paths, &new_key)?;

        // 5. Write rules to new database
        new_dbs.secrets.replace_all_pass_rules(&rules)?;

        // 6. Persist new key file path. Same reasoning as move_vault:
        // abort if the write fails so we don't leave the on-disk config
        // referencing a stale key path while the new vault uses the new key.
        use arclain_db::ConfigDb;
        let cfg_conn = ConfigDb::open(&paths.config_db)?.into_sqlite_db();
        cfg_conn.with_connection(|conn| set_config(conn, "key_file_path", new_key_file_path))?;

        // Update paths
        paths.key_file = Some(PathBuf::from(new_key_file_path));

        Ok((paths, new_dbs, rules))
    }
}
