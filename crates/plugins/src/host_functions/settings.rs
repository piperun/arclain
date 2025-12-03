//! Settings management

use super::HostFunctions;

impl HostFunctions {
    pub(super) fn impl_get_setting(&mut self, key: String) -> Option<String> {
        self.settings.lock().get(&key).cloned()
    }

    pub(super) fn impl_set_setting(&mut self, key: String, value: String) {
        self.settings.lock().insert(key, value);
    }
}
