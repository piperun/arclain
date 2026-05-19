//! Per-tab WASM PluginInstance pool. Lazy-spawn on first request;
//! eager-drop when the owning TabState is dropped.

use anyhow::Result;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;

/// Generic over the instance type so unit tests can use mocks while
/// production code uses `arclain_plugins::PluginInstance`.
///
/// Holds at most one instance per plugin_id. Re-requesting the same id
/// returns the cached Arc; the spawn closure is invoked only on a true
/// miss. Errors from the spawn closure are propagated and leave the
/// slot empty for retry.
///
/// # Thread safety
///
/// The map itself is guarded by a `RwLock` for fast concurrent reads.
/// Each instance is wrapped in `Arc<Mutex<T>>` — matching the existing
/// `PluginManager` pattern — because `PluginInstance` contains
/// non-`Sync` wasmtime internals and can only be accessed with
/// exclusive mutable access anyway.
#[derive(Debug)]
pub struct TabPluginPool<T = arclain_plugins::PluginInstance> {
    instances: RwLock<HashMap<String, Arc<Mutex<T>>>>,
}

impl<T> Default for TabPluginPool<T> {
    fn default() -> Self {
        Self {
            instances: RwLock::new(HashMap::new()),
        }
    }
}

impl<T> TabPluginPool<T> {
    /// Lazy-get-or-spawn the instance for `plugin_id`. Returns the
    /// shared `Arc<Mutex<T>>`. Callers lock the mutex themselves to
    /// access the inner value.
    ///
    /// Cache hit path: read-lock the map, clone the Arc, return.
    /// Cache miss path: drop the read lock, acquire write lock,
    /// double-check (another thread may have spawned concurrently),
    /// invoke `spawn`, store the Arc, return a clone.
    ///
    /// On spawn failure the slot stays empty so a subsequent call
    /// re-attempts.
    pub fn try_get_or_spawn(
        &self,
        plugin_id: &str,
        spawn: impl FnOnce() -> Result<T>,
    ) -> Result<Arc<Mutex<T>>> {
        // Fast path: read lock + cache hit.
        {
            let read = self.instances.read();
            if let Some(arc) = read.get(plugin_id) {
                return Ok(arc.clone());
            }
        }
        // Slow path: write lock + double-check + spawn.
        let mut write = self.instances.write();
        if let Some(arc) = write.get(plugin_id) {
            return Ok(arc.clone());
        }
        let instance = spawn()?;
        let arc = Arc::new(Mutex::new(instance));
        write.insert(plugin_id.to_string(), arc.clone());
        Ok(arc)
    }

    /// Drop the cached instance for `plugin_id` (e.g. after a plugin
    /// reload or user-triggered restart). The next `try_get_or_spawn`
    /// will re-spawn. Existing `Arc<Mutex<T>>` clones held by other
    /// callers remain valid until they're released.
    pub fn drop_instance(&self, plugin_id: &str) {
        self.instances.write().remove(plugin_id);
    }

    /// Drop all cached instances. Equivalent to dropping the pool
    /// itself but without invalidating the pool's identity. Useful
    /// when an app-global event (settings change, plugin manifest
    /// reload) requires reset across tabs.
    pub fn drop_all(&self) {
        self.instances.write().clear();
    }

    /// Number of currently-cached instances. Diagnostic / test use.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.instances.read().len()
    }
}

#[cfg(test)]
#[path = "plugin_instances_tests.rs"]
mod tests;
