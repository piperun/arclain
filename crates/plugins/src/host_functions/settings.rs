//! Settings management

use super::HostFunctions;
use std::sync::atomic::Ordering;

impl HostFunctions {
    pub(super) fn impl_get_setting(&mut self, key: String) -> Option<String> {
        self.settings.lock().get(&key).cloned()
    }

    pub(super) fn impl_set_setting(&mut self, key: String, value: String) {
        self.settings.lock().insert(key, value);
        // Mark the snapshot cache stale (audit P14). Release ordering
        // pairs with the AcqRel swap in PluginManager::get_all_settings.
        self.settings_dirty.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_host() -> HostFunctions {
        HostFunctions::new(
            "test".to_string(),
            std::collections::HashSet::new(),
            0,
            HashMap::new(),
        )
    }

    /// Regression test for P14 from `docs/AUDIT_2026-05-03.md`.
    ///
    /// `PluginManager::get_all_settings` used to lock and full-clone
    /// every plugin's settings hashmap on every call, even for plugins
    /// that hadn't touched their settings since last read. The fix
    /// tracks a `settings_dirty: AtomicBool` per instance — host writes
    /// flip it to `true`, the manager swaps it back to `false` after
    /// snapshotting and uses a cached snapshot otherwise.
    ///
    /// This pins the host-side contract: a fresh `HostFunctions` starts
    /// dirty (so the first snapshot populates the cache), and
    /// `impl_set_setting` flips dirty back to `true` regardless of
    /// whether anyone cleared it in the meantime.
    #[test]
    fn p14_set_setting_marks_dirty() {
        let mut host = make_host();
        // Fresh host starts dirty so the first snapshot reads.
        assert!(host.settings_dirty.load(Ordering::Acquire));

        // Manager-side swap clears the flag.
        host.settings_dirty.store(false, Ordering::Release);
        assert!(!host.settings_dirty.load(Ordering::Acquire));

        // A plugin write must flip it back.
        host.impl_set_setting("k".to_string(), "v".to_string());
        assert!(host.settings_dirty.load(Ordering::Acquire));
        assert_eq!(host.impl_get_setting("k".to_string()).as_deref(), Some("v"));

        // Subsequent writes keep it dirty (idempotent set is fine).
        host.impl_set_setting("k".to_string(), "v2".to_string());
        assert!(host.settings_dirty.load(Ordering::Acquire));
    }
}
