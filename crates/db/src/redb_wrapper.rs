use anyhow::Result;
use redb::Database;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Generic wrapper for Redb database handling thread-safe access.
///
/// `db` is `Option<Database>`, not a bare `Database` -- this is what
/// makes [`Self::close`] a *coordinated* close rather than a per-clone
/// one. `ReDb` derives `Clone` (an `Arc` internally), and every clone
/// shares the same `Mutex<Option<Database>>` slot: dropping *your own*
/// clone is an ordinary reference-count decrement that leaves the
/// database open for as long as any other clone survives, but
/// `close()` empties the *shared slot itself*, so every existing clone
/// (this one, and any other holder anywhere in the process) observes
/// "closed" on its very next access -- see `close()`'s own doc comment
/// for why `arclain_app`'s vault move/rekey needs exactly this.
#[derive(Clone)]
pub struct ReDb {
    db: Arc<Mutex<Option<Database>>>,
}

impl ReDb {
    /// Open or create a Redb database at the specified path
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path)?;
        Ok(Self {
            db: Arc::new(Mutex::new(Some(db))),
        })
    }

    /// Execute a closure with access to the underlying database.
    /// Fails with a plain, unambiguous error once [`Self::close`] has
    /// run (on this clone or any other) -- never a confusing OS-level
    /// file-lock error from trying to use a handle whose backing
    /// `redb::Database` no longer exists.
    pub fn with_connection<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Database) -> Result<R>,
    {
        let guard = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let db = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("database is closed"))?;
        f(db)
    }

    /// Closes the underlying `redb::Database` *for every clone of this
    /// `ReDb`*, releasing its OS-level file lock immediately -- not just
    /// for this handle, for all of them, because the `Option<Database>`
    /// every clone shares is what gets emptied, not just this one
    /// clone's own reference to the `Arc`.
    ///
    /// This is the single-owner coordination primitive `arclain_app`'s
    /// vault move/rekey needs: a long-lived caller elsewhere in the
    /// process (`crates/ui`'s `AppState.dbs`, obtained once via
    /// `ArclainApp::take_legacy_composition` and never dropped just
    /// because the vault is about to move) holds an independent clone of
    /// the *same* `Arc<Mutex<Option<Database>>>` this wraps. Before this
    /// existed, moving/rekeying the vault could only release the file's
    /// OS-level lock if *every* outstanding clone happened to drop at
    /// the right moment -- unreachable in practice with a long-lived UI
    /// mirror alive for the app's entire lifetime. `close()` needs
    /// cooperation from none of those other holders: it acts on the
    /// shared slot directly, so the moment the facade calls it, every
    /// other clone's *next* `with_connection` call fails cleanly with
    /// "database is closed" instead of racing a half-moved file. This
    /// also restores the pre-facade behavior the reviewer asked for: the
    /// facade's own copy and any externally-held mirror now go dark
    /// *together*, exactly like the single-owner `AppState.dbs.take()`
    /// this replaced always did.
    ///
    /// Idempotent: closing an already-closed `ReDb` is a no-op.
    pub fn close(&self) {
        if let Ok(mut guard) = self.db.lock() {
            // The extracted `Database` (if any) drops here, at the end
            // of this statement -- releasing its OS-level lock/mmap
            // before this function returns.
            guard.take();
        }
    }
}
