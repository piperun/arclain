use anyhow::{Context, Result};
use parking_lot::{Mutex, MutexGuard};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKeyIdentity {
    cache_base_dir: PathBuf,
    scoped_key: String,
}

type CacheKeyLockRegistry = HashMap<CacheKeyIdentity, Weak<Mutex<()>>>;
type CacheRootLockRegistry = HashMap<PathBuf, Weak<Mutex<()>>>;
type CacheQuotaLockRegistry = HashMap<PathBuf, Weak<Mutex<()>>>;

static CACHE_KEY_LOCKS: OnceLock<Mutex<CacheKeyLockRegistry>> = OnceLock::new();
static CACHE_ROOT_LOCKS: OnceLock<Mutex<CacheRootLockRegistry>> = OnceLock::new();
static CACHE_QUOTA_LOCKS: OnceLock<Mutex<CacheQuotaLockRegistry>> = OnceLock::new();

fn cache_key_lock_registry() -> &'static Mutex<CacheKeyLockRegistry> {
    CACHE_KEY_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_root_lock_registry() -> &'static Mutex<CacheRootLockRegistry> {
    CACHE_ROOT_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_quota_lock_registry() -> &'static Mutex<CacheQuotaLockRegistry> {
    CACHE_QUOTA_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Process-wide lock for one scoped cache key.
///
/// The canonical cache root makes independently constructed `ContentCache`
/// instances coordinate when they target the same on-disk cache.
pub(crate) struct CacheKeyLock {
    state: Arc<Mutex<()>>,
}

/// Process-wide lock for physical-store and index mutations under one cache
/// root. Per-key locks preserve concurrency during downloads; this shorter
/// root lock closes the commit window where reconciliation could otherwise
/// mistake a just-written, not-yet-indexed blob for a crash orphan.
pub(crate) struct CacheRootLock {
    state: Arc<Mutex<()>>,
}

/// Process-wide coordinator for reservations and quota calculations. This is
/// deliberately separate from the physical-store lock so a non-blocking
/// queued write can reserve capacity while the worker is committing another
/// key's blob and index row.
pub(crate) struct CacheQuotaLock {
    state: Arc<Mutex<()>>,
}

impl CacheQuotaLock {
    pub(crate) fn lock(&self) -> MutexGuard<'_, ()> {
        self.state.lock()
    }
}

impl CacheRootLock {
    pub(crate) fn lock(&self) -> MutexGuard<'_, ()> {
        self.state.lock()
    }
}

impl CacheKeyLock {
    pub(crate) fn lock(&self) -> MutexGuard<'_, ()> {
        self.state.lock()
    }
}

pub(crate) fn cache_key_lock(cache_base_dir: &Path, scoped_key: &str) -> Result<CacheKeyLock> {
    let canonical_base = fs::canonicalize(cache_base_dir)
        .with_context(|| format!("canonicalizing cache directory {cache_base_dir:?}"))?;
    let identity = CacheKeyIdentity {
        cache_base_dir: canonical_base,
        scoped_key: scoped_key.to_string(),
    };

    let mut registry = cache_key_lock_registry().lock();
    registry.retain(|_, weak| weak.strong_count() > 0);
    let state = registry
        .get(&identity)
        .and_then(Weak::upgrade)
        .unwrap_or_else(|| {
            let state = Arc::new(Mutex::new(()));
            registry.insert(identity, Arc::downgrade(&state));
            state
        });
    Ok(CacheKeyLock { state })
}

pub(crate) fn cache_root_lock(cache_base_dir: &Path) -> Result<CacheRootLock> {
    let canonical_base = fs::canonicalize(cache_base_dir)
        .with_context(|| format!("canonicalizing cache directory {cache_base_dir:?}"))?;
    let mut registry = cache_root_lock_registry().lock();
    registry.retain(|_, weak| weak.strong_count() > 0);
    let state = registry
        .get(&canonical_base)
        .and_then(Weak::upgrade)
        .unwrap_or_else(|| {
            let state = Arc::new(Mutex::new(()));
            registry.insert(canonical_base, Arc::downgrade(&state));
            state
        });
    Ok(CacheRootLock { state })
}

pub(crate) fn cache_quota_lock(cache_base_dir: &Path) -> Result<CacheQuotaLock> {
    let canonical_base = fs::canonicalize(cache_base_dir)
        .with_context(|| format!("canonicalizing cache directory {cache_base_dir:?}"))?;
    let mut registry = cache_quota_lock_registry().lock();
    registry.retain(|_, weak| weak.strong_count() > 0);
    let state = registry
        .get(&canonical_base)
        .and_then(Weak::upgrade)
        .unwrap_or_else(|| {
            let state = Arc::new(Mutex::new(()));
            registry.insert(canonical_base, Arc::downgrade(&state));
            state
        });
    Ok(CacheQuotaLock { state })
}
