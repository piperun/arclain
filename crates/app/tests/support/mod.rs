//! Shared helpers for `arclain_app`'s bootstrap integration tests.
//!
//! Every helper here builds a fully isolated, temp-directory-rooted
//! [`arclain_app::AppPaths`] and optionally pre-seeds the files a
//! `bootstrap()` call reads on the way up (`config.sqlite`, a dummy
//! "7-Zip" executable). Nothing here touches a real user profile.

use std::path::{Path, PathBuf};

use arclain_app::AppPaths;

/// Builds an [`AppPaths`] whose five directories are distinct
/// subdirectories of `root` (normally a [`tempfile::TempDir`]'s path).
/// None of the five exist yet -- `bootstrap()` is responsible for
/// creating them, and "first run" tests rely on that.
pub fn temp_paths(root: &Path) -> AppPaths {
    AppPaths {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        log_dir: root.join("logs"),
        plugins_dir: root.join("plugins"),
    }
}

/// Where `bootstrap()` places `config.sqlite`, mirroring the
/// `data_dir/databases/config.sqlite` convention documented on
/// [`AppPaths`]. Test-only mirror of the crate-private
/// `AppPaths::databases_dir` -- kept in exact sync with it by the
/// `paths_documented_layout_matches_test_support` test in
/// `bootstrap.rs`.
pub fn databases_dir(paths: &AppPaths) -> PathBuf {
    paths.data_dir.join("databases")
}

/// Creates an empty file at `dir/name` and returns its path. Used as a
/// stand-in 7-Zip executable: `bootstrap()` never invokes the
/// executable during composition (only real archive operations would),
/// it only needs the path to exist, so a dummy file is enough to make
/// 7-Zip detection succeed deterministically regardless of whether the
/// machine running the test actually has 7-Zip installed.
pub fn create_dummy_executable(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("create dummy executable's parent dir");
    let path = dir.join(name);
    std::fs::write(&path, b"not a real binary, only its path is checked")
        .expect("write dummy executable");
    path
}

/// Pre-seeds a *valid* `config.sqlite` under `paths` with
/// `sevenzip_path` pointing at `sevenzip_path`, so `bootstrap()`'s
/// 7-Zip detection deterministically succeeds via the explicit-path
/// branch instead of searching the real system `PATH` (whose contents
/// this test suite cannot control across machines/CI).
pub fn seed_working_sevenzip_config(paths: &AppPaths, sevenzip_path: &Path) {
    let databases_dir = databases_dir(paths);
    std::fs::create_dir_all(&databases_dir).expect("create databases dir");
    let config_db_path = databases_dir.join("config.sqlite");

    let db = arclain_core::config::ConfigDb::open(&config_db_path).expect("open config db");
    let conn = db.into_sqlite_db();
    conn.with_connection(|conn| {
        arclain_core::UserConfig::ensure_table(conn)?;
        let mut config = arclain_core::UserConfig::new();
        config.sevenzip_path = Some(sevenzip_path.to_string_lossy().into_owned());
        config.save(conn)?;
        Ok(())
    })
    .expect("seed sevenzip_path into config db");
}

/// Overwrites where `config.sqlite` would live with bytes that are not
/// a valid SQLite file, simulating a corrupted configuration database.
pub fn seed_corrupt_config(paths: &AppPaths) {
    let databases_dir = databases_dir(paths);
    std::fs::create_dir_all(&databases_dir).expect("create databases dir");
    std::fs::write(
        databases_dir.join("config.sqlite"),
        b"this is not a sqlite database, just noise to corrupt the file",
    )
    .expect("write corrupt config.sqlite");
}
