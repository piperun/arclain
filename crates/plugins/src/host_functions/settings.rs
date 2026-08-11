//! Settings management

use super::HostFunctions;
use std::sync::atomic::Ordering;

const MAX_SETTING_ENTRIES: usize = 128;
const MAX_SETTING_KEY_BYTES: usize = 128;
const MAX_SETTING_VALUE_BYTES: usize = 64 * 1024;
const MAX_SETTING_TOTAL_BYTES: usize = 1024 * 1024;

/// A whole-map settings replacement that has passed the exact limits the host
/// applies to guest `set-setting` calls. Its fields stay private so callers
/// cannot manufacture an unchecked replacement for a live plugin instance.
#[derive(Clone, Debug)]
pub struct ValidatedPluginSettings {
    values: std::collections::HashMap<String, String>,
}

impl ValidatedPluginSettings {
    pub(crate) fn into_values(self) -> std::collections::HashMap<String, String> {
        self.values
    }
}

/// The stable reason a host-facing whole-map settings replacement was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginSettingsValidationError {
    TooManyEntries,
    KeyTooLong,
    ValueTooLong,
    TotalTooLarge,
}

fn validate_setting_entry(
    settings: &std::collections::HashMap<String, String>,
    key: &str,
    value: &str,
) -> Result<(), PluginSettingsValidationError> {
    if key.len() > MAX_SETTING_KEY_BYTES {
        return Err(PluginSettingsValidationError::KeyTooLong);
    }
    if value.len() > MAX_SETTING_VALUE_BYTES {
        return Err(PluginSettingsValidationError::ValueTooLong);
    }

    let is_existing = settings.contains_key(key);
    let old_entry_bytes = settings
        .get_key_value(key)
        .map_or(0, |(old_key, old_value)| old_key.len() + old_value.len());
    if !is_existing && settings.len() >= MAX_SETTING_ENTRIES {
        return Err(PluginSettingsValidationError::TooManyEntries);
    }
    let retained_bytes = settings
        .iter()
        .map(|(stored_key, stored_value)| stored_key.len() + stored_value.len())
        .sum::<usize>();
    let next_bytes = retained_bytes
        .saturating_sub(old_entry_bytes)
        .saturating_add(key.len())
        .saturating_add(value.len());
    if next_bytes > MAX_SETTING_TOTAL_BYTES {
        return Err(PluginSettingsValidationError::TotalTooLarge);
    }

    Ok(())
}

/// Validates a host-requested whole-map replacement with the same per-entry
/// and aggregate limits that guest `set-setting` calls use.
pub fn validate_plugin_settings(
    settings: std::collections::BTreeMap<String, String>,
) -> Result<ValidatedPluginSettings, PluginSettingsValidationError> {
    let mut validated = std::collections::HashMap::with_capacity(settings.len());
    for (key, value) in settings {
        validate_setting_entry(&validated, &key, &value)?;
        validated.insert(
            key.into_boxed_str().into_string(),
            value.into_boxed_str().into_string(),
        );
    }
    Ok(ValidatedPluginSettings { values: validated })
}

fn insert_bounded_setting(
    settings: &mut std::collections::HashMap<String, String>,
    key: String,
    value: String,
) -> bool {
    if validate_setting_entry(settings, &key, &value).is_err() {
        return false;
    }

    let key = key.into_boxed_str().into_string();
    let value = value.into_boxed_str().into_string();
    settings.insert(key, value);
    true
}

pub(super) fn bounded_initial_settings(
    settings: std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, String> {
    let mut entries = settings.into_iter().collect::<Vec<_>>();
    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    let mut bounded = std::collections::HashMap::new();
    for (key, value) in entries {
        let _ = insert_bounded_setting(&mut bounded, key, value);
    }
    bounded
}

impl HostFunctions {
    pub(super) fn impl_get_setting(&mut self, key: String) -> Option<String> {
        self.settings.lock().get(&key).cloned()
    }

    pub(super) fn impl_set_setting(&mut self, key: String, value: String) {
        let mut settings = self.settings.lock();
        if !insert_bounded_setting(&mut settings, key, value) {
            return;
        }
        drop(settings);
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
        .unwrap()
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

    #[test]
    fn settings_reject_oversized_keys_values_and_new_entries_at_capacity() {
        let mut host = make_host();
        host.settings_dirty.store(false, Ordering::Release);

        host.impl_set_setting("k".repeat(MAX_SETTING_KEY_BYTES + 1), "value".to_string());
        host.impl_set_setting(
            "oversized-value".to_string(),
            "v".repeat(MAX_SETTING_VALUE_BYTES + 1),
        );
        assert!(host.settings.lock().is_empty());
        assert!(!host.settings_dirty.load(Ordering::Acquire));

        for index in 0..128 {
            host.impl_set_setting(format!("key-{index}"), "value".to_string());
        }
        host.settings_dirty.store(false, Ordering::Release);
        host.impl_set_setting("one-too-many".to_string(), "value".to_string());
        assert_eq!(host.settings.lock().len(), 128);
        assert!(!host.settings_dirty.load(Ordering::Acquire));

        host.impl_set_setting("key-0".to_string(), "updated".to_string());
        assert_eq!(
            host.impl_get_setting("key-0".to_string()).as_deref(),
            Some("updated")
        );
        assert!(host.settings_dirty.load(Ordering::Acquire));
    }

    #[test]
    fn settings_bound_aggregate_bytes_and_reallocate_guest_capacity() {
        let mut host = make_host();
        let mut oversized_capacity = String::with_capacity(1024 * 1024);
        oversized_capacity.push_str("small-value");
        host.impl_set_setting("small-key".to_string(), oversized_capacity);

        {
            let settings = host.settings.lock();
            assert!(settings["small-key"].capacity() <= MAX_SETTING_VALUE_BYTES);
        }

        for index in 0..128 {
            host.impl_set_setting(
                format!("aggregate-{index}"),
                "x".repeat(MAX_SETTING_VALUE_BYTES),
            );
        }
        let settings = host.settings.lock();
        let retained = settings
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum::<usize>();
        assert!(retained <= MAX_SETTING_TOTAL_BYTES);
    }

    /// Catches a host-facing settings validator that silently truncates an
    /// over-bound form instead of refusing the whole replacement.
    #[test]
    fn validated_plugin_settings_reject_an_over_bound_map() {
        let over_bound = (0..=MAX_SETTING_ENTRIES)
            .map(|index| (format!("key-{index:03}"), "value".to_string()))
            .collect();

        assert!(
            crate::validate_plugin_settings(over_bound).is_err(),
            "a replacement map with more than {MAX_SETTING_ENTRIES} entries must be rejected"
        );
    }

    #[test]
    fn initial_settings_are_deterministically_bounded() {
        let mut initial = HashMap::new();
        initial.insert("000-valid".to_string(), "kept".to_string());
        initial.insert(
            "z".repeat(MAX_SETTING_KEY_BYTES + 1),
            "rejected".to_string(),
        );
        for index in 0..=MAX_SETTING_ENTRIES {
            initial.insert(format!("entry-{index:03}"), "value".to_string());
        }

        let bounded = bounded_initial_settings(initial);

        assert!(bounded.len() <= MAX_SETTING_ENTRIES);
        assert!(bounded.contains_key("000-valid"));
        assert!(bounded.keys().all(|key| key.len() <= MAX_SETTING_KEY_BYTES));
    }
}
