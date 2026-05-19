//! Tests for tab persistence. Loaded via `#[path]` so `super::*` is
//! the contents of persistence.rs.

use super::*;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn snapshot_captures_order_and_active() {
    let mut col = TabsCollection::new();
    col.open(Some(PathBuf::from("/tmp/a.zip")));
    col.open(Some(PathBuf::from("/tmp/b.zip")));
    let snap = snapshot(&col);
    assert_eq!(snap.version, 1);
    assert_eq!(snap.tabs.len(), 3); // initial empty + a + b
    assert_eq!(snap.tabs[1].archive_path, Some(PathBuf::from("/tmp/a.zip")));
    assert_eq!(snap.tabs[2].archive_path, Some(PathBuf::from("/tmp/b.zip")));
}

#[test]
fn round_trip_preserves_order_and_active() {
    let snap = TabsSnapshot {
        version: 1,
        tabs: vec![
            TabRestore { id: TabId(1), archive_path: Some(PathBuf::from("/a.zip")) },
            TabRestore { id: TabId(5), archive_path: Some(PathBuf::from("/b.zip")) },
            TabRestore { id: TabId(7), archive_path: None },
        ],
        active: TabId(5),
        next_id: 8,
    };
    let json = serde_json::to_string(&snap).unwrap();
    let parsed: TabsSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, snap);
}

#[test]
fn restore_to_collection_recreates_tabs_and_active() {
    let snap = TabsSnapshot {
        version: 1,
        tabs: vec![
            TabRestore { id: TabId(1), archive_path: Some(PathBuf::from("/a.zip")) },
            TabRestore { id: TabId(2), archive_path: None },
        ],
        active: TabId(2),
        next_id: 3,
    };
    let col = restore_collection(snap);
    assert_eq!(col.tabs().len(), 2);
    assert_eq!(col.active_id(), TabId(2));
    assert_eq!(
        col.get(TabId(1)).unwrap().archive_path.get(),
        Some(PathBuf::from("/a.zip"))
    );
    assert_eq!(col.peek_next_id(), 3);
}

#[test]
fn restore_with_invalid_active_falls_back_to_first() {
    let snap = TabsSnapshot {
        version: 1,
        tabs: vec![
            TabRestore { id: TabId(1), archive_path: None },
            TabRestore { id: TabId(2), archive_path: None },
        ],
        active: TabId(99), // not in tabs
        next_id: 3,
    };
    let col = restore_collection(snap);
    assert_eq!(col.active_id(), TabId(1));
}

#[test]
fn restore_with_empty_snapshot_seeds_single_empty_tab() {
    let snap = TabsSnapshot {
        version: 1,
        tabs: vec![],
        active: TabId(1),
        next_id: 1,
    };
    let col = restore_collection(snap);
    assert_eq!(col.tabs().len(), 1);
    assert!(col.active().archive_path.get().is_none());
}

#[test]
fn save_and_load_round_trip_via_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tabs.json");
    let mut col = TabsCollection::new();
    col.open(Some(PathBuf::from("/saved.zip")));
    save_collection(&col, &path).unwrap();
    assert!(path.exists());
    let restored = load_collection(&path).unwrap();
    assert_eq!(restored.tabs().len(), col.tabs().len());
    assert_eq!(restored.active_id(), col.active_id());
}

#[test]
fn load_missing_file_returns_err() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("nonexistent.json");
    let result = load_collection(&path);
    assert!(result.is_err());
}

#[test]
fn load_corrupt_json_returns_err() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("corrupt.json");
    std::fs::write(&path, "{ this is not valid json").unwrap();
    let result = load_collection(&path);
    assert!(result.is_err());
}

#[test]
fn save_creates_parent_dirs() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("nested/sub/dir/tabs.json");
    let col = TabsCollection::new();
    save_collection(&col, &path).unwrap();
    assert!(path.exists());
}
