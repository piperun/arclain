use super::*;
use arclain_app::archive::{ArchivePath, EntryKind};
use arclain_app::ids::EntryId;

fn dto(id: u64, path: &str, kind: EntryKind) -> ArchiveEntryDto {
    ArchiveEntryDto {
        id: EntryId::from_raw(id),
        path: ArchivePath::parse(path.to_string()).unwrap(),
        name: path.rsplit('/').next().unwrap_or(path).to_string(),
        kind,
        compressed_size: Some(5),
        uncompressed_size: 10,
        modified_at_unix_ms: Some(1_700_000_000_000),
        encrypted: false,
        crc32: Some("DEADBEEF".to_string()),
    }
}

fn inventory_of(session: u64, revision: u64, paths: &[&str]) -> ArchiveInventory {
    ArchiveInventory {
        session_id: ArchiveSessionId::from_raw(session),
        revision,
        entries: paths
            .iter()
            .enumerate()
            .map(|(index, path)| dto(index as u64 + 1, path, EntryKind::File))
            .collect(),
    }
}

fn bound_inventory() -> TabInventory {
    TabInventory::for_session(Some(ArchiveSessionId::from_raw(1)))
}

#[test]
fn a_fresh_inventory_has_no_rows_and_a_zero_count() {
    let inventory = bound_inventory();
    assert_eq!(inventory.revision(), None);
    assert!(inventory.entries().is_empty());
    assert_eq!(inventory.entry_count(), 0);
    assert!(inventory.legacy_rows().is_empty());
}

/// An empty tab's legacy projection keeps one shared identity across
/// reads -- the tree-projection cache keys on `Arc::ptr_eq`, so a fresh
/// allocation per frame would rebuild the tree every frame.
#[test]
fn an_empty_inventorys_legacy_rows_keep_a_stable_arc_identity() {
    let inventory = bound_inventory();
    assert!(Arc::ptr_eq(
        &inventory.legacy_rows(),
        &TabInventory::default().legacy_rows()
    ));
}

#[test]
fn adopting_seats_the_rows_and_the_derived_legacy_projection_together() {
    let mut inventory = bound_inventory();
    assert!(inventory.adopt(AdoptedInventory::prepare(inventory_of(
        1,
        1,
        &["readme.txt", "game/data.bin"],
    ))));

    assert_eq!(inventory.revision(), Some(1));
    assert_eq!(inventory.entry_count(), 2);
    let legacy = inventory.legacy_rows();
    assert_eq!(legacy.len(), 2);

    // The projection is `core_entry_from_dto` row for row -- the same
    // conversion every remaining core-typed consumer reads through.
    // (`ArchiveEntry` has no `PartialEq`, so the comparison is per field.)
    for (dto, converted) in inventory.entries().iter().zip(legacy.iter()) {
        let expected = crate::core::utils::core_entry_from_dto(dto);
        assert_eq!(converted.path, expected.path);
        assert_eq!(converted.size, expected.size);
        assert_eq!(converted.packed_size, expected.packed_size);
        assert_eq!(converted.modified, expected.modified);
        assert_eq!(converted.is_dir, expected.is_dir);
        assert_eq!(converted.encrypted, expected.encrypted);
        assert_eq!(converted.crc32, expected.crc32);
    }
}

/// The load-bearing guard: rows carry session-scoped `EntryId`s, so a
/// late answer from the archive the tab held *before* must never seat
/// under the new binding, however new its revision looks.
#[test]
fn an_inventory_from_another_session_is_refused_however_new_it_looks() {
    let mut inventory = bound_inventory();
    assert!(inventory.adopt(AdoptedInventory::prepare(inventory_of(
        1,
        1,
        &["this-archive.txt"],
    ))));

    assert!(!inventory.adopt(AdoptedInventory::prepare(inventory_of(
        2,
        99,
        &["other-archive.txt"],
    ))));
    assert_eq!(inventory.entries()[0].path.as_str(), "this-archive.txt");
}

#[test]
fn a_sessionless_inventory_adopts_nothing() {
    let mut inventory = TabInventory::default();
    assert!(!inventory.adopt(AdoptedInventory::prepare(inventory_of(1, 1, &["a.txt"]))));
    assert_eq!(inventory.entry_count(), 0);
}

/// Two racing refreshes converge on the higher revision regardless of
/// reply order; an equal-revision refetch re-seats identical rows.
#[test]
fn an_inventory_older_than_the_one_held_is_refused_and_an_equal_one_reseats() {
    let mut inventory = bound_inventory();
    assert!(inventory.adopt(AdoptedInventory::prepare(inventory_of(
        1,
        5,
        &["current.txt"]
    ))));

    assert!(!inventory.adopt(AdoptedInventory::prepare(inventory_of(
        1,
        4,
        &["stale.txt"]
    ))));
    assert_eq!(inventory.entries()[0].path.as_str(), "current.txt");

    assert!(inventory.adopt(AdoptedInventory::prepare(inventory_of(
        1,
        5,
        &["same-revision.txt"],
    ))));
    assert!(inventory.adopt(AdoptedInventory::prepare(inventory_of(
        1,
        6,
        &["newer.txt"]
    ))));
    assert_eq!(inventory.revision(), Some(6));
}

/// Rebinding to a new session starts over: the old rows are gone and the
/// new session's first answer (revision 1 again) seats.
#[test]
fn rebinding_to_a_new_session_lets_its_first_inventory_seat() {
    let mut inventory = bound_inventory();
    assert!(inventory.adopt(AdoptedInventory::prepare(inventory_of(1, 7, &["old.txt"]))));

    inventory = TabInventory::for_session(Some(ArchiveSessionId::from_raw(2)));
    assert!(inventory.entries().is_empty());
    assert!(inventory.adopt(AdoptedInventory::prepare(inventory_of(2, 1, &["new.txt"]))));
    assert_eq!(inventory.entries()[0].path.as_str(), "new.txt");
}

/// A directory row's conversion keeps the folder-defining fields the
/// legacy consumers key on.
#[test]
fn the_legacy_projection_preserves_the_directory_flag_and_aggregates() {
    let mut inventory = bound_inventory();
    let mut fetched = inventory_of(1, 1, &["game/data.bin"]);
    fetched.entries.insert(
        0,
        ArchiveEntryDto {
            uncompressed_size: 10,
            compressed_size: Some(5),
            ..dto(9, "game", EntryKind::Directory)
        },
    );
    assert!(inventory.adopt(AdoptedInventory::prepare(fetched)));

    let legacy = inventory.legacy_rows();
    let game = legacy.iter().find(|entry| entry.path == "game").unwrap();
    assert!(game.is_dir);
    assert_eq!(game.size, 10, "the index's recursive aggregate rides along");
    assert_eq!(game.packed_size, 5);
    assert_eq!(game.crc32.as_deref(), Some("DEADBEEF"));
}
