use super::{archive_entry_count, archive_entry_page, HostFunctions, MAX_ARCHIVE_PAGE_ITEMS};
use crate::active_tab::ActiveTabBridge;
use crate::arclain::plugin::host::Host;
use crate::types::PluginCapability;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[test]
fn archive_page_is_bounded_and_uses_stable_offsets() {
    let entries = (0..600)
        .map(|index| arclain_core::ArchiveEntry {
            path: format!("folder/file-{index:04}.bin"),
            size: index as u64,
            packed_size: index as u64,
            is_dir: false,
            encrypted: false,
            modified: None,
            crc32: None,
        })
        .collect::<Vec<_>>();

    assert_eq!(archive_entry_count(&entries), 600);
    assert_eq!(
        archive_entry_page(&entries, 255, 2).unwrap(),
        vec![
            "folder/file-0255.bin".to_string(),
            "folder/file-0256.bin".to_string(),
        ]
    );
    assert!(archive_entry_page(&entries, 0, (MAX_ARCHIVE_PAGE_ITEMS + 1) as u32).is_err());
}

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
