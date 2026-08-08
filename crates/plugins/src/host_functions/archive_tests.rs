use super::{archive_entry_count, archive_entry_page, HostFunctions, MAX_ARCHIVE_PAGE_ITEMS};
use crate::active_tab::ActiveTabBridge;
use crate::types::PluginCapability;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use wirt::bindings::wirt::plugin::host::Host;

#[test]
fn archive_page_is_bounded_and_uses_stable_offsets() {
    let entries = (0..600)
        .map(|index| format!("folder/file-{index:04}.bin"))
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

/// A per-event context answers `list_archive_files`/`archive_file_count`
/// from the originating session's own entry paths, not from whichever
/// archive the active-tab bridge happens to be showing when the worker
/// gets around to the event. That routing is the whole reason the
/// context carries an entry list at all.
#[test]
fn an_event_context_answers_archive_listing_from_its_own_entries() {
    use crate::host_functions::EventContext;

    let bridge = Arc::new(TestActiveTabBridge::default());
    bridge.set_archive_path(Some("C:/library/whatever-is-active.zip".to_string()));
    let capabilities = HashSet::from([PluginCapability::ArchiveMetadataRead]);
    let directory = tempfile::tempdir().expect("create test directory");
    let mut host = HostFunctions::new_with_plugin_log_dir(
        "event-context-listing".to_string(),
        capabilities,
        0,
        HashMap::new(),
        directory.path(),
    )
    .expect("construct host functions");
    host.set_active_tab_bridge(bridge.clone());
    host.set_event_context(Some(EventContext {
        archive_path: "C:/library/the-event-archive.zip".to_string(),
        password: None,
        entries: Arc::new(vec![
            "docs".to_string(),
            "docs/manual.txt".to_string(),
            "readme.txt".to_string(),
        ]),
        archive_session_id: 7,
    }));

    assert_eq!(
        Host::list_archive_files(&mut host).expect("the event context supplies the listing"),
        vec![
            "docs".to_string(),
            "docs/manual.txt".to_string(),
            "readme.txt".to_string(),
        ],
        "the guest must see the event's own entries, in the order they were captured"
    );
    assert_eq!(
        Host::archive_file_count(&mut host).expect("the event context supplies the count"),
        3,
        "the count is this list's length -- directories included, as the listing itself is"
    );
    assert_eq!(
        Host::current_archive_info(&mut host)
            .expect("the event context supplies the archive")
            .filename,
        "the-event-archive.zip",
        "the event's archive wins over whatever the bridge is showing"
    );
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
    metadata: parking_lot::Mutex<Option<serde_json::Value>>,
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

    fn active_archive_session_id(&self) -> Option<u64> {
        None
    }

    fn set_session_metadata(&self, _archive_session_id: u64, metadata: Option<serde_json::Value>) {
        *self.metadata.lock() = metadata;
    }

    fn set_active_tab_metadata(&self, metadata: Option<serde_json::Value>) {
        *self.metadata.lock() = metadata;
    }

    fn set_archive_path(&self, path: Option<String>) {
        *self.archive_path.lock() = path;
    }
}
