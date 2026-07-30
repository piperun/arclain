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
    assert!(inventory.entries_arc().is_empty());
}

/// An empty tab's row list keeps one shared identity across reads -- the
/// tree-projection cache keys on `Arc::ptr_eq`, so a fresh allocation
/// per frame would rebuild the tree every frame.
#[test]
fn an_empty_inventorys_rows_keep_a_stable_arc_identity() {
    let inventory = bound_inventory();
    assert!(Arc::ptr_eq(
        &inventory.entries_arc(),
        &TabInventory::default().entries_arc()
    ));
}

#[test]
fn adopting_seats_the_sessions_rows_verbatim() {
    let mut inventory = bound_inventory();
    let fetched = inventory_of(1, 1, &["readme.txt", "game/data.bin"]);
    let expected = fetched.entries.clone();
    assert!(inventory.adopt(AdoptedInventory::prepare(fetched)));

    assert_eq!(inventory.revision(), Some(1));
    assert_eq!(inventory.entry_count(), 2);
    assert_eq!(inventory.entries(), expected.as_slice());
}

/// Two reads of the same adopted inventory hand out the same allocation,
/// so the renderer's tree projection sees "unchanged" rather than
/// rebuilding the folder tree every frame.
#[test]
fn reading_the_rows_twice_hands_out_the_same_allocation() {
    let mut inventory = bound_inventory();
    assert!(inventory.adopt(AdoptedInventory::prepare(inventory_of(1, 1, &["a.txt"]))));

    assert!(Arc::ptr_eq(
        &inventory.entries_arc(),
        &inventory.entries_arc()
    ));
}

/// The plugin bridge hands its event context this list verbatim, so its
/// order and membership are what a plugin guest sees from
/// `list_archive_files`: every entry the session indexed, directories
/// included, in the facade's own depth-first order.
#[test]
fn adopting_seats_the_entry_paths_in_the_facades_own_order() {
    let mut inventory = bound_inventory();
    let mut fetched = inventory_of(1, 1, &["game/data.bin", "readme.txt"]);
    fetched
        .entries
        .insert(0, dto(9, "game", EntryKind::Directory));
    assert!(inventory.adopt(AdoptedInventory::prepare(fetched)));

    assert_eq!(
        inventory.entry_paths().as_slice(),
        ["game", "game/data.bin", "readme.txt"],
    );
    assert_eq!(
        inventory.entry_paths().len(),
        inventory.entry_count(),
        "one path per indexed entry -- the count a guest reads is this list's length"
    );
}

/// An empty tab's path list keeps one shared identity across reads, for
/// the same per-frame-allocation reason the row list does.
#[test]
fn an_empty_inventorys_entry_paths_keep_a_stable_arc_identity() {
    let inventory = bound_inventory();
    assert!(inventory.entry_paths().is_empty());
    assert!(Arc::ptr_eq(
        &inventory.entry_paths(),
        &TabInventory::default().entry_paths()
    ));
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

/// A directory row arrives with the index's own recursive aggregates,
/// which is what the tree panel and the browser's folder rows draw.
#[test]
fn a_directory_row_keeps_its_flag_and_the_indexs_aggregates() {
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

    let game = inventory
        .entries()
        .iter()
        .find(|entry| entry.path.as_str() == "game")
        .unwrap();
    assert_eq!(game.kind, EntryKind::Directory);
    assert_eq!(
        game.uncompressed_size, 10,
        "the index's recursive aggregate rides along"
    );
    assert_eq!(game.compressed_size, Some(5));
    assert_eq!(game.crc32.as_deref(), Some("DEADBEEF"));
}
