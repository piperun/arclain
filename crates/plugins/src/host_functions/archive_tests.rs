use super::HostFunctions;
use crate::active_tab::ActiveTabBridge;
use crate::arclain::plugin::host::Host;
use crate::types::PluginCapability;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[test]
fn rename_does_not_replace_an_existing_destination() {
    let directory = tempfile::tempdir().expect("create test directory");
    let source = directory.path().join("source.cbz");
    let destination = directory.path().join("destination.cbz");
    std::fs::write(&source, b"source archive").expect("write source");
    std::fs::write(&destination, b"destination archive").expect("write destination");
    let bridge = Arc::new(TestActiveTabBridge::default());
    bridge.set_archive_path(Some(source.to_string_lossy().into_owned()));
    let capabilities = HashSet::from([PluginCapability::ArchiveModify]);
    let mut host = HostFunctions::new_with_plugin_log_dir(
        "archive-collision-test".to_string(),
        capabilities,
        0,
        HashMap::new(),
        directory.path(),
    )
    .expect("construct host functions");
    host.set_active_tab_bridge(bridge.clone());

    let result = Host::rename_archive(&mut host, "destination.cbz".to_string());

    assert_eq!(
        result.expect_err("an existing destination must reject the rename"),
        "A file named 'destination.cbz' already exists"
    );
    assert_eq!(
        std::fs::read(&source).expect("source must remain after collision"),
        b"source archive"
    );
    assert_eq!(
        std::fs::read(&destination).expect("destination must remain after collision"),
        b"destination archive"
    );
    assert_eq!(
        bridge.archive_path().as_deref(),
        Some(source.to_string_lossy().as_ref()),
        "a failed rename must leave the active archive path unchanged"
    );
}

#[derive(Default)]
struct TestActiveTabBridge {
    archive_path: parking_lot::Mutex<Option<String>>,
    metadata: arclain_signals::Signal<Option<serde_json::Value>>,
}

impl ActiveTabBridge for TestActiveTabBridge {
    fn archive_path(&self) -> Option<String> {
        self.archive_path.lock().clone()
    }

    fn current_password(&self) -> Option<String> {
        None
    }

    fn archive_entries(&self) -> Vec<String> {
        Vec::new()
    }

    fn metadata_signal(&self) -> arclain_signals::Signal<Option<serde_json::Value>> {
        self.metadata.clone()
    }

    fn set_archive_path(&self, path: Option<String>) {
        *self.archive_path.lock() = path;
    }
}
