//! Database bootstrap: canonical paths and open-everything entry point.
//!
//! Extracted out of `lib.rs` (audit module-org callout) so the
//! `DbPaths` type and `open_databases` orchestration sit together
//! away from the crate-root re-export bookkeeping.

use crate::cache::CacheDb;
use crate::config::ConfigDb;
use crate::metadata_store::MetadataStore;
use crate::pool::DieselPool;
use crate::secrets::SecretsDb;
use crate::secrets_key::SecretsKey;
use crate::SqliteDb;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Canonical paths for the two databases and optional key-file
#[derive(Debug, Clone)]
pub struct DbPaths {
    pub config_db: PathBuf,
    pub cache_db: PathBuf,
    pub secrets_db: PathBuf,
    pub key_file: Option<PathBuf>,
}

impl DbPaths {
    /// Calculate default paths without creating them.
    /// Creation is now handled by arclain_core::dirs::AppDirectories.
    pub fn calculate_defaults(app_name: &str) -> Result<Self> {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(app_name);

        let databases_dir = base.join("databases");
        let secrets_dir = base.join("secrets");

        Ok(Self {
            config_db: databases_dir.join("config.sqlite"),
            cache_db: databases_dir.join("metadata.sqlite"),
            secrets_db: secrets_dir.join("pass.redb"),
            key_file: Some(secrets_dir.join("master.key")),
        })
    }
}

/// Holds open connections to both databases.
///
/// `Clone`: every field is already a cheap, `Arc`-backed handle
/// (`SqliteDb`, `SecretsDb`, `MetadataStore` each wrap an `Arc<Mutex<_>>`
/// or pool internally; `DieselPool` is itself `Clone`), so cloning this
/// struct never opens a second physical connection -- every clone
/// observes the same writes as every other. `arclain_app`'s settings
/// facade relies on this: it retains one authoritative live clone
/// alongside the one handed to `crates/ui`'s legacy `AppState`, so a
/// mutation through either is immediately visible through the other.
#[derive(Clone)]
pub struct ConfigDbs {
    pub config: SqliteDb,
    pub secrets: SecretsDb,
    pub metadata: MetadataStore,
    pub config_pool: DieselPool,
    pub cache_pool: DieselPool,
}

/// Drop leftover replication triggers from the cache database.
///
/// A database written by an older build carries triggers from the
/// since-removed cr-sqlite sync layer. Their bodies call
/// `crsql_internal_sync_bit`, a function nothing registers on the
/// connection any more, so every INSERT and UPDATE that fires one fails
/// outright. Arclain itself creates no triggers on this file, so any
/// trigger present is such a leftover and is dropped.
///
/// This belongs at the shared open rather than in any one consumer's
/// constructor: the cache index, the content cache and the metadata
/// backend all write through this same file, so a repair owned by one of
/// them leaves the others broken whenever that one is not constructed.
///
/// Best effort by design. A file that cannot be opened or queried here is
/// left to `CacheDb::open`'s own corrupt-file recovery, and the connection
/// is dropped before that runs so the file stays replaceable on Windows. A
/// database that does not exist yet is left alone rather than created.
fn drop_stale_sync_triggers(db_path: &Path) {
    if !db_path.exists() {
        return;
    }
    let Ok(conn) = crate::DbConnection::open(db_path) else {
        return;
    };

    let triggers: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='trigger'")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get(0))
                .map(|rows| rows.filter_map(|row| row.ok()).collect())
        })
        .unwrap_or_default();

    for trigger in &triggers {
        // Double any embedded quote so the name stays inside its
        // identifier: a raw interpolation would let a trigger named
        // `evil"quote` close the identifier early, fail the drop with the
        // error swallowed below, and still be counted as dropped.
        let quoted_trigger = trigger.replace('"', "\"\"");
        let _ = conn.execute_batch(&format!("DROP TRIGGER IF EXISTS \"{quoted_trigger}\""));
    }

    if !triggers.is_empty() {
        tracing::info!(
            "[Bootstrap] Dropped {} stale triggers from the cache database",
            triggers.len()
        );
    }
}

/// Open all databases, initializing schemas if needed
pub fn open_databases(paths: &DbPaths, key: &SecretsKey) -> Result<ConfigDbs> {
    // Open config database using new module
    let config_db = ConfigDb::open(&paths.config_db)
        .with_context(|| format!("Failed to open config database at {:?}", paths.config_db))?;

    // Create Diesel pool for config
    let config_pool = DieselPool::new(&paths.config_db)
        .with_context(|| "Failed to create config database pool")?;

    // Repair the cache database before anything opens a handle on it.
    drop_stale_sync_triggers(&paths.cache_db);

    // Open cache database
    let cache_db = CacheDb::open(&paths.cache_db)
        .with_context(|| format!("Failed to open cache database at {:?}", paths.cache_db))?;

    // Create Diesel pool for cache
    let cache_pool =
        DieselPool::new(&paths.cache_db).with_context(|| "Failed to create cache database pool")?;

    // Open secrets database using new module
    let secrets_db = SecretsDb::open(&paths.secrets_db, &key.as_bytes())
        .with_context(|| format!("Failed to open secrets database at {:?}", paths.secrets_db))?;

    Ok(ConfigDbs {
        config: config_db.into_sqlite_db(),
        secrets: secrets_db,
        metadata: MetadataStore::new(
            cache_db.into_sqlite_db(),
            cache_pool.clone(),
            paths
                .cache_db
                .parent()
                .unwrap_or(Path::new("."))
                .join("metadata"),
            Some(paths.cache_db.clone()),
        )
        .with_context(|| {
            format!(
                "Failed to initialize metadata store at {:?}",
                paths.cache_db
            )
        })?,
        config_pool,
        cache_pool,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DbConnection;

    /// A cache database left behind by an older build carries cr-sqlite
    /// replication triggers whose bodies call `crsql_internal_sync_bit`.
    /// Nothing registers that function any more, so every write that fires
    /// one fails -- and every consumer of this file writes through it, not
    /// just the metadata backend. Opening the databases must clear them,
    /// whatever else the process goes on to construct.
    ///
    /// One of the seeded triggers carries a double quote in its name, which
    /// SQLite accepts and the repair must therefore survive: dropping it
    /// takes a doubled quote in the identifier, and an unescaped `DROP
    /// TRIGGER` leaves it in place -- silently, since the drop error is
    /// swallowed. Both triggers sit on the same table, so a survivor keeps
    /// blocking the write this test makes at the end.
    #[test]
    fn open_databases_clears_stale_sync_triggers_from_the_cache_database() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = DbPaths {
            config_db: temp.path().join("config.sqlite"),
            cache_db: temp.path().join("metadata.sqlite"),
            secrets_db: temp.path().join("secrets.redb"),
            key_file: None,
        };

        // Seed the cache database the way an older build left it, and
        // prove the triggers really do break writes before the repair.
        {
            let seeded = DbConnection::open(&paths.cache_db).unwrap();
            seeded
                .execute_batch(
                    r#"CREATE TABLE legacy_rows (id INTEGER PRIMARY KEY, value TEXT);
                       CREATE TRIGGER legacy_rows_sync AFTER INSERT ON legacy_rows
                       BEGIN
                           SELECT crsql_internal_sync_bit();
                       END;
                       CREATE TRIGGER "evil""quote_trigger" AFTER INSERT ON legacy_rows
                       BEGIN
                           SELECT crsql_internal_sync_bit();
                       END;"#,
                )
                .unwrap();

            let seeded_triggers: i64 = seeded
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                seeded_triggers, 2,
                "the fixture must plant both a plain and a quoted-name trigger"
            );

            let blocked = seeded
                .execute("INSERT INTO legacy_rows (value) VALUES ('before')", [])
                .expect_err("the stale triggers must break writes before the repair");
            assert!(
                blocked.to_string().contains("crsql_internal_sync_bit"),
                "unexpected pre-repair failure: {blocked}"
            );
        }

        let dbs = open_databases(&paths, &SecretsKey::generate()).unwrap();

        let conn = DbConnection::open(&paths.cache_db).unwrap();
        let surviving: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='trigger'")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap();
        assert!(
            surviving.is_empty(),
            "stale triggers survived the shared open: {surviving:?}"
        );

        // The write the triggers used to block now goes through...
        conn.execute("INSERT INTO legacy_rows (value) VALUES ('after')", [])
            .expect("writes must work once the stale triggers are gone");

        // ...as does one on a table created after the open, through the
        // shared handle every cache consumer writes on.
        dbs.metadata
            .db()
            .with_connection(|conn| {
                conn.execute_batch(
                    "CREATE TABLE post_open_rows (id INTEGER PRIMARY KEY, value TEXT);
                     INSERT INTO post_open_rows (value) VALUES ('after');",
                )?;
                Ok(())
            })
            .expect("the shared cache handle must accept writes after the repair");
    }
}
