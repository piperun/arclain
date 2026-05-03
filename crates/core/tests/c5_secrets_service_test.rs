//! Regression test for C5 from `docs/AUDIT_2026-05-03.md`.
//!
//! Pre-fix, `SecretsService::move_vault` used `let _ =` to discard
//! `set_config` failures after copying the secrets DB to the new path
//! and updating in-memory state. If the config write failed, the user
//! had a copied secrets DB but on-disk config still pointed at the old
//! path; the next launch opened the wrong DB (potentially empty or with
//! a different vault key).
//!
//! Post-fix, `move_vault` propagates the config-write error so the user
//! can fix the cause and retry. The destination copy is left in place
//! for the user to reconcile.
//!
//! Force the failure by pre-creating `app_config` with a wrong column
//! schema. `ConfigDb::open`'s `CREATE TABLE IF NOT EXISTS` is a no-op
//! when the table already exists, so the corrupt schema survives.
//! `set_config`'s `INSERT INTO app_config(key, value)` then fails with
//! "no such column: value", which `move_vault` (post-fix) surfaces.

use arclain_core::services::SecretsService;
use arclain_db::{DbConnection, DbPaths, SecretsDb, SecretsKey};
use std::path::PathBuf;
use tempfile::TempDir;

fn corrupt_app_config_schema(path: &std::path::Path) {
    let conn = DbConnection::open(path).expect("opening sqlite for setup");
    conn.execute(
        "CREATE TABLE app_config (key TEXT PRIMARY KEY, wrong_col TEXT)",
        [],
    )
    .expect("creating mis-schema'd app_config");
    drop(conn);
}

fn build_paths(temp: &TempDir, secrets_filename: &str) -> (DbPaths, SecretsKey, PathBuf) {
    let key = SecretsKey::generate();
    let key_path = temp.path().join("key.bin");
    key.save_to_file(&key_path).unwrap();

    let secrets_src = temp.path().join(secrets_filename);
    // Materialize a real redb file at src so `fs::copy` has something to copy.
    let _ = SecretsDb::open(&secrets_src, &key.as_bytes()).unwrap();

    let config_db = temp.path().join("config.sqlite");
    corrupt_app_config_schema(&config_db);

    let cache_db = temp.path().join("cache.sqlite");

    let paths = DbPaths {
        config_db,
        cache_db,
        secrets_db: secrets_src,
        key_file: Some(key_path.clone()),
    };
    (paths, key, key_path)
}

#[test]
fn c5_move_vault_propagates_set_config_failure() {
    let temp = TempDir::new().unwrap();
    let (paths, _key, _key_path) = build_paths(&temp, "secrets_src.redb");
    let dst = temp.path().join("secrets_dst.redb");

    let mut paths_opt = Some(paths);
    let result = SecretsService::move_vault(&mut paths_opt, dst.to_str().unwrap());

    assert!(
        result.is_err(),
        "C5 fix regressed: move_vault returned Ok despite the config-DB write failing. \
         The on-disk config still points at the old path; the next launch would open \
         the wrong vault.",
    );

    // The destination copy is intentionally left in place — the user (or
    // a higher layer) decides whether to clean up or retry.
    assert!(
        dst.exists(),
        "Sanity: destination copy should be in place even though move_vault aborted",
    );
}
