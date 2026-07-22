//! Persistent record of every pipeline execution — input hash + pipeline
//! config hash → output path + status.
//!
//! Used for:
//! - **Idempotent re-runs.** Before a pipeline runs its producing steps, the
//!   executor looks up `(input_blake3, pipeline_hash)`; if a `completed` row
//!   exists and its output is still on disk, the run is skipped.
//! - **Crash recovery.** Rows linger in `in_progress` only while a pipeline
//!   is executing. On startup, any stale ones get flipped to `failed`.
//! - **Audit trail.** "What did arclain do to this archive, and when?"
//!
//! Schema is created via `ensure_pipeline_runs_table`; call it from whatever
//! opens the config DB (see `ConfigDb::init_schema`).

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// Status values for `pipeline_runs.status`. Kept as string constants rather
/// than an enum so raw SQL stays readable.
pub mod status {
    pub const IN_PROGRESS: &str = "in_progress";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";
}

/// Output kind values. Matches the on-disk artifact type the pipeline produced.
pub mod output_kind {
    pub const ARCHIVE: &str = "archive";
    pub const FOLDER: &str = "folder";
}

/// Canonical `interrupted` marker written into `error` when a stale
/// `in_progress` row is reaped on startup.
pub const INTERRUPTED_MARKER: &str = "interrupted";

/// A row from `pipeline_runs`.
#[derive(Debug, Clone)]
pub struct DbPipelineRun {
    pub id: i64,
    pub input_path: String,
    pub input_blake3: String,
    pub input_size: i64,
    pub pipeline_hash: String,
    pub output_path: Option<String>,
    pub output_kind: Option<String>,
    pub status: String,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
    pub arclain_version: String,
}

/// Arguments for inserting a fresh `in_progress` row.
pub struct NewPipelineRun<'a> {
    pub input_path: &'a str,
    pub input_blake3: &'a str,
    pub input_size: i64,
    pub pipeline_hash: &'a str,
    pub arclain_version: &'a str,
}

/// Create the `pipeline_runs` table and its indices. Idempotent.
pub fn ensure_pipeline_runs_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS pipeline_runs (
            id              INTEGER PRIMARY KEY,
            input_path      TEXT    NOT NULL,
            input_blake3    TEXT    NOT NULL,
            input_size      INTEGER NOT NULL,
            pipeline_hash   TEXT    NOT NULL,
            output_path     TEXT,
            output_kind     TEXT,
            status          TEXT    NOT NULL,
            started_at      INTEGER NOT NULL,
            completed_at    INTEGER,
            error           TEXT,
            arclain_version TEXT    NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_pipeline_runs_lookup
            ON pipeline_runs(input_blake3, pipeline_hash, status);
        CREATE INDEX IF NOT EXISTS idx_pipeline_runs_output
            ON pipeline_runs(output_path);
        "#,
    )?;
    Ok(())
}

fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Insert a new `in_progress` row. Returns the row id, which should be passed
/// to `mark_run_completed` or `mark_run_failed` once the pipeline finishes.
pub fn begin_pipeline_run(conn: &Connection, new: &NewPipelineRun<'_>) -> Result<i64> {
    conn.execute(
        "INSERT INTO pipeline_runs
            (input_path, input_blake3, input_size, pipeline_hash,
             status, started_at, arclain_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            new.input_path,
            new.input_blake3,
            new.input_size,
            new.pipeline_hash,
            status::IN_PROGRESS,
            unix_now(),
            new.arclain_version,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn mark_run_completed(
    conn: &Connection,
    id: i64,
    output_path: &str,
    output_kind: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE pipeline_runs
         SET status = ?1, output_path = ?2, output_kind = ?3, completed_at = ?4, error = NULL
         WHERE id = ?5",
        params![status::COMPLETED, output_path, output_kind, unix_now(), id],
    )?;
    Ok(())
}

pub fn mark_run_failed(conn: &Connection, id: i64, error: &str) -> Result<()> {
    conn.execute(
        "UPDATE pipeline_runs
         SET status = ?1, completed_at = ?2, error = ?3
         WHERE id = ?4",
        params![status::FAILED, unix_now(), error, id],
    )?;
    Ok(())
}

/// Look up the most recent `completed` row for a given input+pipeline.
/// Returns `None` if there's no prior successful run.
pub fn find_completed_run(
    conn: &Connection,
    input_blake3: &str,
    pipeline_hash: &str,
) -> Result<Option<DbPipelineRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, input_path, input_blake3, input_size, pipeline_hash,
                output_path, output_kind, status, started_at, completed_at, error, arclain_version
         FROM pipeline_runs
         WHERE input_blake3 = ?1 AND pipeline_hash = ?2 AND status = ?3
         ORDER BY completed_at DESC
         LIMIT 1",
    )?;
    let row = stmt
        .query_row(
            params![input_blake3, pipeline_hash, status::COMPLETED],
            map_row,
        )
        .optional()?;
    Ok(row)
}

/// Mark any `in_progress` rows whose `started_at` is older than
/// `stale_threshold_secs` seconds ago as `failed` with error
/// `INTERRUPTED_MARKER`. Run this at service startup — if the process was
/// alive the rows would have already been finalized.
///
/// Returns the number of rows flipped.
pub fn flag_stale_in_progress(conn: &Connection, stale_threshold_secs: i64) -> Result<usize> {
    let cutoff = unix_now() - stale_threshold_secs;
    let rows = conn.execute(
        "UPDATE pipeline_runs
         SET status = ?1, completed_at = ?2, error = ?3
         WHERE status = ?4 AND started_at < ?5",
        params![
            status::FAILED,
            unix_now(),
            INTERRUPTED_MARKER,
            status::IN_PROGRESS,
            cutoff,
        ],
    )?;
    Ok(rows)
}

/// Return rows with `error = INTERRUPTED_MARKER` completed since `since_unix`.
/// Used by the UI to surface a "previous runs interrupted" banner.
pub fn list_interrupted_since(conn: &Connection, since_unix: i64) -> Result<Vec<DbPipelineRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, input_path, input_blake3, input_size, pipeline_hash,
                output_path, output_kind, status, started_at, completed_at, error, arclain_version
         FROM pipeline_runs
         WHERE error = ?1 AND completed_at IS NOT NULL AND completed_at >= ?2
         ORDER BY completed_at DESC",
    )?;
    let rows = stmt.query_map(params![INTERRUPTED_MARKER, since_unix], map_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DbPipelineRun> {
    Ok(DbPipelineRun {
        id: row.get(0)?,
        input_path: row.get(1)?,
        input_blake3: row.get(2)?,
        input_size: row.get(3)?,
        pipeline_hash: row.get(4)?,
        output_path: row.get(5)?,
        output_kind: row.get(6)?,
        status: row.get(7)?,
        started_at: row.get(8)?,
        completed_at: row.get(9)?,
        error: row.get(10)?,
        arclain_version: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_pipeline_runs_table(&conn).unwrap();
        conn
    }

    fn new_run<'a>(blake3: &'a str, pipeline: &'a str) -> NewPipelineRun<'a> {
        NewPipelineRun {
            input_path: "/tmp/mod.rar",
            input_blake3: blake3,
            input_size: 1024,
            pipeline_hash: pipeline,
            arclain_version: "0.10.0",
        }
    }

    #[test]
    fn roundtrip_begin_complete() {
        let conn = open_test_db();
        let id = begin_pipeline_run(&conn, &new_run("abc", "pipe1")).unwrap();

        let found = find_completed_run(&conn, "abc", "pipe1").unwrap();
        assert!(
            found.is_none(),
            "in_progress rows don't match completed lookup"
        );

        mark_run_completed(&conn, id, "/tmp/mod.zip", output_kind::ARCHIVE).unwrap();

        let found = find_completed_run(&conn, "abc", "pipe1").unwrap().unwrap();
        assert_eq!(found.output_path.as_deref(), Some("/tmp/mod.zip"));
        assert_eq!(found.output_kind.as_deref(), Some(output_kind::ARCHIVE));
        assert_eq!(found.status, status::COMPLETED);
        assert!(found.error.is_none());
    }

    #[test]
    fn failed_runs_dont_appear_in_completed_lookup() {
        let conn = open_test_db();
        let id = begin_pipeline_run(&conn, &new_run("xyz", "pipe1")).unwrap();
        mark_run_failed(&conn, id, "boom").unwrap();
        assert!(find_completed_run(&conn, "xyz", "pipe1").unwrap().is_none());
    }

    #[test]
    fn different_pipeline_hash_does_not_match() {
        let conn = open_test_db();
        let id = begin_pipeline_run(&conn, &new_run("abc", "pipe1")).unwrap();
        mark_run_completed(&conn, id, "/tmp/out.zip", output_kind::ARCHIVE).unwrap();

        // Same input, different pipeline config
        assert!(find_completed_run(&conn, "abc", "pipe2").unwrap().is_none());
    }

    #[test]
    fn flag_stale_flips_old_in_progress_rows() {
        let conn = open_test_db();

        // Backdate a row to ~1 hour ago
        conn.execute(
            "INSERT INTO pipeline_runs
                (input_path, input_blake3, input_size, pipeline_hash, status, started_at, arclain_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "/tmp/old.rar",
                "old",
                1,
                "pipe",
                status::IN_PROGRESS,
                unix_now() - 3700,
                "0.10.0",
            ],
        )
        .unwrap();

        // Fresh row (should NOT be flipped)
        let fresh_id = begin_pipeline_run(&conn, &new_run("fresh", "pipe")).unwrap();

        let flipped = flag_stale_in_progress(&conn, 3600).unwrap();
        assert_eq!(flipped, 1);

        // Fresh row untouched
        let fresh_status: String = conn
            .query_row(
                "SELECT status FROM pipeline_runs WHERE id = ?1",
                [fresh_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fresh_status, status::IN_PROGRESS);

        // Interrupted row surfaces in list_interrupted_since
        let interrupted = list_interrupted_since(&conn, 0).unwrap();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].error.as_deref(), Some(INTERRUPTED_MARKER));
    }
}
