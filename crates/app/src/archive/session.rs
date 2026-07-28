//! One open archive: its backend handle, its indexed entries, and the
//! read-only queries (`list_entries`, `snapshot`) the facade serves from
//! it.
//!
//! [`EntryIndex`] is built once per revision (at open time; a future
//! mutation-aware task rebuilds it and bumps [`ArchiveSession::revision`]
//! after any change). Building it is the only place archive-entry paths
//! are walked and folder totals aggregated -- `list_entries` itself is a
//! pure in-memory sort/filter/paginate over the already-built index, so
//! repeated queries (pagination, re-sorting) never re-touch the backend.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::archive::{
    ArchiveEntryDto, ArchivePath, ArchiveSnapshot, EntryKind, EntryPage, EntrySortKey,
    ListEntriesRequest, SortDirection,
};
use crate::ids::{ArchiveSessionId, EntryId};

/// Mints a fresh, process-wide-unique [`EntryId`]. Ids are revision-scoped
/// in spirit (see the module doc comment): a new [`EntryIndex::build`]
/// call -- whether for the initial open or a future reindex after a
/// mutation -- always mints fresh ids from this counter rather than
/// deriving them from content, so a stale id from a superseded revision
/// can never coincidentally match a live entry in a newer one.
fn next_entry_id() -> EntryId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    EntryId::from_raw(NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Days since the Unix epoch for a proleptic-Gregorian civil date.
/// Howard Hinnant's `days_from_civil` algorithm (public domain) -- correct
/// across the full range we need without pulling in a date/time crate for
/// what is otherwise a single best-effort parse.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (i64::from(month) + 9) % 12; // [0, 11]: Mar=0 .. Feb=11
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Best-effort parse of a backend-reported `modified` string into a Unix
/// millisecond timestamp.
///
/// Every backend in this workspace that populates `ArchiveEntry::modified`
/// formats it as `"{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:
/// {second:02}"` (see `arclain_core::backends::zip_backend::ZipBackend::
/// list`) -- a fixed-width, zero-padded shape that sorts identically
/// whether compared as a string or parsed to a number. Parsing to a real
/// timestamp (rather than keeping the pre-facade UI's raw string compare)
/// is therefore a pure representation change, not a behavior one, and it
/// is what lets `ArchiveEntryDto::modified_at_unix_ms` be a real `i64`
/// instead of leaking a display-formatted string into the frontend-neutral
/// DTO. Any string that does not match this shape yields `None` -- which
/// sorts before every real timestamp (`Option<i64>`'s derived `Ord` puts
/// `None` first), matching the pre-facade UI's own "no modified date"
/// entries (an empty string, which also sorts first).
fn parse_modified_to_unix_ms(value: &str) -> Option<i64> {
    let (date_part, time_part) = value.split_once(' ')?;
    let mut date_fields = date_part.split('-');
    let year: i64 = date_fields.next()?.parse().ok()?;
    let month: u32 = date_fields.next()?.parse().ok()?;
    let day: u32 = date_fields.next()?.parse().ok()?;
    if date_fields.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut time_fields = time_part.split(':');
    let hour: i64 = time_fields.next()?.parse().ok()?;
    let minute: i64 = time_fields.next()?.parse().ok()?;
    let second: i64 = time_fields.next()?.parse().ok()?;
    if time_fields.next().is_some() || hour >= 24 || minute >= 60 || second >= 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    Some(seconds * 1000)
}

/// Reproduces the pre-facade UI's `SortColumn::Type` key exactly (see
/// `crates/ui/src/shared/models/file_entry.rs::sort_entry_indices`):
/// directories always sort under the literal string `"directory"`, which
/// is NOT a "folders first" grouping -- it interleaves alphabetically with
/// file extensions (a file extension earlier in the alphabet than
/// "directory", such as `.bat`, sorts before every folder in ascending
/// order). Reproduced as-is rather than "fixed" because this task
/// characterizes existing behavior, not redesigns it.
fn kind_sort_key(dto: &ArchiveEntryDto) -> String {
    match dto.kind {
        EntryKind::Directory => "directory".to_string(),
        // `arclain_core::ArchiveEntry` has no symlink concept today, so
        // this arm is unreached in practice; treated identically to a
        // file (extension-keyed) since no prior behavior exists to match.
        EntryKind::File | EntryKind::Symlink => {
            // `rsplit('.').next()` is `Some` for every non-empty string
            // (including one with no '.', which yields the whole name) --
            // matching the pre-facade code's exact behavior, including its
            // practically-dead `.unwrap_or("file")` fallback (kept for
            // fidelity, not because it can trigger).
            dto.name.rsplit('.').next().unwrap_or("file").to_lowercase()
        }
    }
}

/// One entry discovered while walking a backend's flat entry list, before
/// folder totals are aggregated. Distinct from [`ArchiveEntryDto`]: this is
/// a mutable working value `EntryIndex::build` accumulates into, not the
/// immutable DTO a caller receives.
struct RawEntry {
    uncompressed_size: u64,
    compressed_size: u64,
    modified: Option<String>,
    encrypted: bool,
    crc32: Option<String>,
}

/// Normalizes a backend-reported path the same way `ArchivePath::parse`
/// would (backslashes to forward slashes), additionally trimming a single
/// trailing slash some backends use to mark an explicit directory entry
/// (e.g. `"folder/"`), so it matches the ancestor-directory paths this
/// module synthesizes (which never carry a trailing slash).
fn normalize(raw: &str) -> String {
    raw.replace('\\', "/").trim_end_matches('/').to_string()
}

/// Every proper ancestor directory of `path`, root-to-leaf excluded
/// (`path` itself is not its own ancestor), shallowest first. Matches
/// `arclain_core::archive::NavigationState::get_all_folders`'s walk.
fn ancestors(path: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = path;
    while let Some(pos) = current.rfind('/') {
        current = &current[..pos];
        result.push(current.to_string());
    }
    result.reverse();
    result
}

/// The parent directory of `path` (`""` for a root-level entry).
fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(pos) => &path[..pos],
        None => "",
    }
}

/// The basename of `path`.
fn basename(path: &str) -> &str {
    match path.rfind('/') {
        Some(pos) => &path[pos + 1..],
        None => path,
    }
}

/// The indexed contents of one archive session: every entry (file and
/// folder, at every depth) keyed by its minted [`EntryId`], plus each
/// directory's ordered child-id list. Built once per revision by
/// [`EntryIndex::build`]; `ArchiveSession::list_entries` sorts/filters/
/// paginates this in memory without touching the backend again.
#[derive(Debug)]
pub(crate) struct EntryIndex {
    by_id: HashMap<EntryId, ArchiveEntryDto>,
    /// Each directory's children, name-sorted at build time (matching the
    /// pre-facade UI's baseline order: `NavigationState::
    /// filter_entries_with_archive_paths` fed its per-folder entries
    /// through a `BTreeMap` keyed by relative path before any user sort
    /// was applied, so two entries tied on a chosen sort key retain this
    /// alphabetical relative order -- reproduced here via a stable sort
    /// by name at build time, which `list_entries`'s own stable sort
    /// preserves for tied keys).
    children: HashMap<ArchivePath, Vec<EntryId>>,
    entry_count: u64,
    total_uncompressed_size: u64,
}

impl EntryIndex {
    /// Builds an index from a backend's flat entry list. Synthesizes any
    /// ancestor directory a file implies but the backend did not list
    /// explicitly (matching `NavigationState::get_all_folders`), and
    /// aggregates each directory's total size/compressed-size across every
    /// descendant file at any depth (matching `NavigationState::
    /// compute_folder_totals`/`compute_folder_crc`'s recursive-prefix
    /// aggregation) so a folder's row in `list_entries` sorts and displays
    /// identically to the pre-facade UI's folder rows.
    ///
    /// Entries whose path is rejected by [`ArchivePath::parse`] (NUL
    /// bytes, absolute, or containing a `..` segment -- never legitimate
    /// in an archive-relative path) are skipped rather than failing the
    /// whole index: one adversarial or corrupt entry should not hide the
    /// rest of an otherwise-listable archive.
    ///
    /// Duplicate paths (two backend entries reporting the identical
    /// path -- a malformed or adversarial archive, not a normal one) are
    /// preserved as distinct rows with distinct [`EntryId`]s rather than
    /// collapsed: the second occurrence does not overwrite the first.
    pub(crate) fn build(entries: &[arclain_core::ArchiveEntry]) -> Self {
        let mut files: Vec<(String, RawEntry)> = Vec::new();
        let mut dirs: std::collections::BTreeMap<String, RawEntry> =
            std::collections::BTreeMap::new();

        for entry in entries {
            let canonical = normalize(&entry.path);
            if ArchivePath::parse(canonical.clone()).is_err() {
                continue;
            }
            if canonical.is_empty() {
                // The archive root itself is never a listed child of
                // anything; a backend reporting an empty/root entry has
                // nothing meaningful to display.
                continue;
            }
            let raw = RawEntry {
                uncompressed_size: entry.size,
                compressed_size: entry.packed_size,
                modified: entry.modified.clone(),
                encrypted: entry.encrypted,
                crc32: entry.crc32.clone(),
            };
            if entry.is_dir {
                dirs.entry(canonical).or_insert(raw);
            } else {
                files.push((canonical, raw));
            }
            // Ancestors are synthesized in a second pass below, after
            // every real entry (file or explicit directory) is known --
            // synthesizing while iterating could otherwise race an
            // explicit directory entry that appears later in the list.
        }

        let mut implied_dirs: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for (path, _) in &files {
            implied_dirs.extend(ancestors(path));
        }
        for path in dirs.keys() {
            implied_dirs.extend(ancestors(path));
        }
        for path in implied_dirs {
            dirs.entry(path).or_insert(RawEntry {
                uncompressed_size: 0,
                compressed_size: 0,
                modified: None,
                encrypted: false,
                crc32: None,
            });
        }

        // Aggregate each directory's totals across every descendant file
        // at any depth (recursive-prefix aggregation, matching
        // `compute_folder_totals`/`compute_folder_crc`).
        let mut by_id: HashMap<EntryId, ArchiveEntryDto> = HashMap::new();
        let mut children: HashMap<ArchivePath, Vec<EntryId>> = HashMap::new();
        let mut entry_count: u64 = 0;
        let mut total_uncompressed_size: u64 = 0;

        // Files: duplicates preserved, ordinal = encounter order for a
        // given path. Sorting by path is a stable sort, so entries
        // sharing a path keep their encounter-order relative position --
        // the "collision ordinal" the id derivation is keyed on.
        let mut ordered_files = files;
        ordered_files.sort_by(|a, b| a.0.cmp(&b.0));

        for (path, raw) in ordered_files {
            let id = next_entry_id();
            let name = basename(&path).to_string();
            let parent = ArchivePath::parse(parent_of(&path).to_string())
                .unwrap_or_else(|_| ArchivePath::root());
            entry_count += 1;
            total_uncompressed_size = total_uncompressed_size.saturating_add(raw.uncompressed_size);
            let dto = ArchiveEntryDto {
                id,
                path: ArchivePath::parse(path).unwrap_or_else(|_| ArchivePath::root()),
                name,
                kind: EntryKind::File,
                compressed_size: Some(raw.compressed_size),
                uncompressed_size: raw.uncompressed_size,
                modified_at_unix_ms: raw.modified.as_deref().and_then(parse_modified_to_unix_ms),
                encrypted: raw.encrypted,
                crc32: raw.crc32,
            };
            by_id.insert(id, dto);
            children.entry(parent).or_default().push(id);
        }

        for (path, raw) in dirs {
            let (total_size, total_packed, folder_crc) = aggregate_folder(&by_id, &path);
            let id = next_entry_id();
            let name = basename(&path).to_string();
            let parent = ArchivePath::parse(parent_of(&path).to_string())
                .unwrap_or_else(|_| ArchivePath::root());
            entry_count += 1;
            let dto = ArchiveEntryDto {
                id,
                path: ArchivePath::parse(path).unwrap_or_else(|_| ArchivePath::root()),
                name,
                kind: EntryKind::Directory,
                compressed_size: Some(total_packed),
                uncompressed_size: total_size,
                modified_at_unix_ms: raw.modified.as_deref().and_then(parse_modified_to_unix_ms),
                encrypted: raw.encrypted,
                crc32: folder_crc,
            };
            by_id.insert(id, dto);
            children.entry(parent).or_default().push(id);
        }

        for ids in children.values_mut() {
            ids.sort_by(|a, b| {
                by_id[a]
                    .name
                    .to_lowercase()
                    .cmp(&by_id[b].name.to_lowercase())
            });
        }

        Self {
            by_id,
            children,
            entry_count,
            total_uncompressed_size,
        }
    }

    fn children_of(&self, directory: &ArchivePath) -> &[EntryId] {
        self.children
            .get(directory)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn entry_count(&self) -> u64 {
        self.entry_count
    }

    pub(crate) fn total_uncompressed_size(&self) -> u64 {
        self.total_uncompressed_size
    }
}

/// Sums uncompressed size, compressed size, and a combined CRC-32 across
/// every entry already indexed under `folder_path/...` (any depth).
/// Mirrors `NavigationState::compute_folder_totals`/`compute_folder_crc`
/// exactly, including the CRC construction (sorted `path:crc` pairs hashed
/// together, order-independent of backend listing order).
fn aggregate_folder(
    by_id: &HashMap<EntryId, ArchiveEntryDto>,
    folder_path: &str,
) -> (u64, u64, Option<String>) {
    let prefix = format!("{folder_path}/");
    let mut size = 0u64;
    let mut packed = 0u64;
    let mut crc_pairs: Vec<(String, String)> = Vec::new();

    for dto in by_id.values() {
        if dto.kind != EntryKind::File {
            continue;
        }
        let path = dto.path.as_str();
        if path == folder_path || path.starts_with(&prefix) {
            size = size.saturating_add(dto.uncompressed_size);
            packed = packed.saturating_add(dto.compressed_size.unwrap_or(0));
            if let Some(crc) = &dto.crc32 {
                crc_pairs.push((path.to_string(), crc.to_uppercase()));
            }
        }
    }

    let crc = if crc_pairs.is_empty() {
        None
    } else {
        crc_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut hasher = crc32fast::Hasher::new();
        for (path, crc) in crc_pairs {
            hasher.update(path.as_bytes());
            hasher.update(b":");
            hasher.update(crc.as_bytes());
            hasher.update(b"\n");
        }
        Some(format!("{:08X}", hasher.finalize()))
    };

    (size, packed, crc)
}

/// One open archive: its backend handle (for future read/mutate
/// operations to reuse), its indexed entries, and the metadata a
/// [`ArchiveSnapshot`] reports.
#[derive(Debug)]
pub(crate) struct ArchiveSession {
    id: ArchiveSessionId,
    source_path: PathBuf,
    archive_type: String,
    archive: Arc<Mutex<arclain_core::Archive>>,
    revision: AtomicU64,
    entry_index: RwLock<EntryIndex>,
}

impl ArchiveSession {
    pub(crate) fn new(
        id: ArchiveSessionId,
        source_path: PathBuf,
        archive_type: String,
        archive: arclain_core::Archive,
        entries: &[arclain_core::ArchiveEntry],
    ) -> Self {
        Self {
            id,
            source_path,
            archive_type,
            archive: Arc::new(Mutex::new(archive)),
            revision: AtomicU64::new(1),
            entry_index: RwLock::new(EntryIndex::build(entries)),
        }
    }

    pub(crate) fn id(&self) -> ArchiveSessionId {
        self.id
    }

    /// Not read by anything this task adds: a future extract/mutate
    /// operation is the intended caller (it needs the archive's original
    /// source path alongside the backend handle `archive_arc` returns).
    #[allow(dead_code)]
    pub(crate) fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Clones the session's backend handle `Arc` for a caller that needs
    /// to perform archive I/O (extraction, mutation) -- callers must
    /// release any store/session lock guard before invoking a blocking
    /// backend call through this handle (see the crate's runtime/executor
    /// rules); nothing in this type enforces that itself.
    ///
    /// Not called by anything this task adds (open/close/list/snapshot are
    /// all read-only queries that never need the backend handle again
    /// once the session is indexed); kept as the seam a future extract/
    /// mutate task reuses rather than re-deriving its own way to reach the
    /// backend a session already resolved.
    #[allow(dead_code)]
    pub(crate) fn archive_arc(&self) -> Arc<Mutex<arclain_core::Archive>> {
        self.archive.clone()
    }

    pub(crate) fn snapshot(&self) -> ArchiveSnapshot {
        let index = self.entry_index.read();
        ArchiveSnapshot {
            session_id: self.id,
            revision: self.revision(),
            source_path: self.source_path.clone(),
            archive_type: self.archive_type.clone(),
            entry_count: index.entry_count(),
            total_uncompressed_size: index.total_uncompressed_size(),
            // Neither `arclain_core::archive::ArchiveInfo` nor any backend
            // in this workspace reports an archive comment or free-form
            // metadata today; always `None` until a future task adds a
            // real source for either.
            comment: None,
            metadata: None,
        }
    }

    pub(crate) fn list_entries(&self, request: &ListEntriesRequest) -> EntryPage {
        let index = self.entry_index.read();
        let revision = self.revision();
        let children = index.children_of(&request.directory);

        let filter = request
            .name_filter
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(str::to_lowercase);

        let mut matching: Vec<&ArchiveEntryDto> = children
            .iter()
            .filter_map(|id| index.by_id.get(id))
            .filter(|dto| {
                filter
                    .as_deref()
                    .is_none_or(|needle| dto.name.to_lowercase().contains(needle))
            })
            .collect();

        match request.sort_key {
            EntrySortKey::Name => {
                matching.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            EntrySortKey::Kind => {
                matching.sort_by(|a, b| kind_sort_key(a).cmp(&kind_sort_key(b)));
            }
            EntrySortKey::Size => {
                matching.sort_by_key(|dto| dto.uncompressed_size);
            }
            EntrySortKey::Modified => {
                matching.sort_by_key(|dto| dto.modified_at_unix_ms);
            }
        }
        if request.sort_direction == SortDirection::Descending {
            matching.reverse();
        }

        let total = matching.len() as u64;
        let entries = matching
            .into_iter()
            .skip(request.offset as usize)
            .take(request.limit as usize)
            .cloned()
            .collect();

        EntryPage {
            session_id: self.id,
            revision,
            directory: request.directory.clone(),
            total,
            entries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64, packed: u64) -> arclain_core::ArchiveEntry {
        arclain_core::ArchiveEntry {
            path: path.to_string(),
            size,
            packed_size: packed,
            modified: None,
            is_dir: false,
            encrypted: false,
            crc32: None,
        }
    }

    fn file_with_modified(path: &str, modified: &str) -> arclain_core::ArchiveEntry {
        arclain_core::ArchiveEntry {
            path: path.to_string(),
            size: 1,
            packed_size: 1,
            modified: Some(modified.to_string()),
            is_dir: false,
            encrypted: false,
            crc32: None,
        }
    }

    fn request(directory: &str) -> ListEntriesRequest {
        ListEntriesRequest {
            directory: ArchivePath::parse(directory.to_string()).unwrap(),
            sort_key: EntrySortKey::Name,
            sort_direction: SortDirection::Ascending,
            name_filter: None,
            offset: 0,
            limit: 1000,
        }
    }

    #[test]
    fn root_listing_shows_top_level_files_and_synthesized_folders() {
        let entries = vec![
            file("readme.txt", 10, 10),
            file("game/Game.exe", 100, 90),
            file("game/data/file.dat", 200, 150),
        ];
        let index = EntryIndex::build(&entries);
        let page = ArchiveSession {
            id: ArchiveSessionId::from_raw(1),
            source_path: PathBuf::from("a.zip"),
            archive_type: "zip".to_string(),
            archive: Arc::new(Mutex::new(dummy_archive())),
            revision: AtomicU64::new(1),
            entry_index: RwLock::new(index),
        }
        .list_entries(&request(""));

        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["game", "readme.txt"]);
        assert_eq!(page.entries[0].kind, EntryKind::Directory);
        assert_eq!(page.total, 2);
    }

    #[test]
    fn nested_listing_shows_only_direct_children_of_the_requested_directory() {
        let entries = vec![
            file("readme.txt", 10, 10),
            file("game/Game.exe", 100, 90),
            file("game/data/file.dat", 200, 150),
        ];
        let session = session_with(&entries);

        let page = session.list_entries(&request("game"));
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["data", "Game.exe"]);
    }

    #[test]
    fn folder_aggregates_descendant_size_and_compressed_size_recursively() {
        let entries = vec![
            file("game/Game.exe", 100, 90),
            file("game/data/file.dat", 200, 150),
        ];
        let session = session_with(&entries);

        let page = session.list_entries(&request(""));
        let folder = page.entries.iter().find(|e| e.name == "game").unwrap();
        assert_eq!(folder.uncompressed_size, 300);
        assert_eq!(folder.compressed_size, Some(240));
    }

    #[test]
    fn duplicate_paths_are_preserved_as_distinct_entries_with_distinct_ids() {
        let entries = vec![file("dup.txt", 1, 1), file("dup.txt", 2, 2)];
        let session = session_with(&entries);

        let page = session.list_entries(&request(""));
        assert_eq!(page.total, 2);
        assert_eq!(page.entries.len(), 2);
        assert_ne!(page.entries[0].id, page.entries[1].id);
        let sizes: Vec<u64> = page.entries.iter().map(|e| e.uncompressed_size).collect();
        assert_eq!(
            sizes,
            [1, 2],
            "encounter order preserved for colliding paths"
        );
    }

    #[test]
    fn entry_ids_are_stable_across_repeated_queries_of_the_same_revision() {
        let entries = vec![file("a.txt", 1, 1), file("b.txt", 2, 2)];
        let session = session_with(&entries);

        let first = session.list_entries(&request(""));
        let second = session.list_entries(&request(""));
        assert_eq!(first.entries[0].id, second.entries[0].id);
        assert_eq!(first.entries[1].id, second.entries[1].id);
    }

    #[test]
    fn rebuilding_the_index_mints_fresh_ids_never_reusing_the_prior_revisions() {
        let entries = vec![file("a.txt", 1, 1)];
        let first_index = EntryIndex::build(&entries);
        let first_id = *first_index
            .children_of(&ArchivePath::root())
            .first()
            .unwrap();
        let second_index = EntryIndex::build(&entries);
        let second_id = *second_index
            .children_of(&ArchivePath::root())
            .first()
            .unwrap();

        assert_ne!(
            first_id, second_id,
            "a reindex must never reuse a prior revision's id"
        );
    }

    #[test]
    fn sort_by_name_is_case_insensitive() {
        let entries = vec![file("Banana.txt", 1, 1), file("apple.txt", 1, 1)];
        let session = session_with(&entries);

        let page = session.list_entries(&request(""));
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["apple.txt", "Banana.txt"]);
    }

    #[test]
    fn sort_by_kind_uses_the_literal_directory_string_not_a_folders_first_grouping() {
        // ".bat" sorts alphabetically before the literal string
        // "directory" ("bat" < "directory"), so ascending Kind sort must
        // place the file before the folder -- reproducing the pre-facade
        // UI's `SortColumn::Type` exactly (no folders-first grouping
        // exists there either).
        let entries = vec![file("script.bat", 1, 1), file("folder/inner.txt", 1, 1)];
        let session = session_with(&entries);

        let mut req = request("");
        req.sort_key = EntrySortKey::Kind;
        let page = session.list_entries(&req);
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["script.bat", "folder"]);
    }

    #[test]
    fn sort_by_size_orders_by_exact_bytes_including_aggregated_folders() {
        let entries = vec![file("small.txt", 5, 5), file("big/only.txt", 500, 500)];
        let session = session_with(&entries);

        let mut req = request("");
        req.sort_key = EntrySortKey::Size;
        let page = session.list_entries(&req);
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["small.txt", "big"]);
    }

    #[test]
    fn sort_by_modified_none_sorts_before_any_real_timestamp_ascending() {
        // Folders never carry a `modified` string from any backend, so
        // they sort first ascending -- matching the pre-facade UI, where
        // a folder's `FileEntry::modified` is always an empty string,
        // which also sorts before any populated date string.
        let entries = vec![
            file_with_modified("dated.txt", "2024-01-15 10:00:00"),
            file("nested/inner.txt", 1, 1),
        ];
        let session = session_with(&entries);

        let mut req = request("");
        req.sort_key = EntrySortKey::Modified;
        let page = session.list_entries(&req);
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["nested", "dated.txt"]);
    }

    #[test]
    fn modified_strings_parse_to_a_monotonically_increasing_timestamp() {
        let earlier = parse_modified_to_unix_ms("2024-01-15 10:00:00").unwrap();
        let later = parse_modified_to_unix_ms("2024-01-15 10:00:01").unwrap();
        assert!(later > earlier);
        assert_eq!(later - earlier, 1000);
    }

    #[test]
    fn unparseable_modified_string_yields_none() {
        assert_eq!(parse_modified_to_unix_ms("not a date"), None);
        assert_eq!(parse_modified_to_unix_ms(""), None);
    }

    #[test]
    fn name_filter_matches_case_insensitively_after_trimming() {
        let entries = vec![file("Report.TXT", 1, 1), file("other.doc", 1, 1)];
        let session = session_with(&entries);

        let mut req = request("");
        req.name_filter = Some("  report ".to_string());
        let page = session.list_entries(&req);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].name, "Report.TXT");
    }

    #[test]
    fn total_counts_entries_after_filtering_not_before() {
        let entries = vec![
            file("a.txt", 1, 1),
            file("b.txt", 1, 1),
            file("a2.txt", 1, 1),
        ];
        let session = session_with(&entries);

        let mut req = request("");
        req.name_filter = Some("a".to_string());
        let page = session.list_entries(&req);
        assert_eq!(page.total, 2);
        assert_eq!(page.entries.len(), 2);
    }

    #[test]
    fn pagination_slices_after_sort_and_filter() {
        let entries = vec![
            file("a.txt", 1, 1),
            file("b.txt", 1, 1),
            file("c.txt", 1, 1),
        ];
        let session = session_with(&entries);

        let mut req = request("");
        req.offset = 1;
        req.limit = 1;
        let page = session.list_entries(&req);
        assert_eq!(
            page.total, 3,
            "total reflects the full filtered set, not the page"
        );
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].name, "b.txt");
    }

    #[test]
    fn unknown_directory_yields_an_empty_page_not_an_error() {
        let entries = vec![file("a.txt", 1, 1)];
        let session = session_with(&entries);

        let page = session.list_entries(&request("no/such/folder"));
        assert_eq!(page.total, 0);
        assert!(page.entries.is_empty());
    }

    #[test]
    fn snapshot_reports_aggregate_totals_across_the_whole_archive() {
        let entries = vec![file("a.txt", 10, 5), file("dir/b.txt", 20, 15)];
        let session = session_with(&entries);
        let snapshot = session.snapshot();
        assert_eq!(snapshot.entry_count, 3); // a.txt, dir, dir/b.txt
        assert_eq!(snapshot.total_uncompressed_size, 30);
        assert_eq!(snapshot.revision, 1);
    }

    fn dummy_archive() -> arclain_core::Archive {
        struct NoopBackend;
        impl arclain_core::ArchiveBackend for NoopBackend {
            fn name(&self) -> &str {
                "noop"
            }
            fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
                arclain_core::archive::BackendCapabilities::read_only()
            }
            fn identify(&self, _path: &Path) -> anyhow::Result<arclain_core::archive::ArchiveKind> {
                Ok(arclain_core::archive::ArchiveKind::Zip)
            }
            fn list(
                &self,
                _path: &Path,
                _password: Option<&str>,
            ) -> anyhow::Result<arclain_core::ArchiveInfo> {
                unimplemented!("not exercised by these tests")
            }
            fn extract_all(
                &self,
                _path: &Path,
                _dest: &Path,
                _password: Option<&str>,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn extract_files(
                &self,
                _path: &Path,
                _dest: &Path,
                _files: &[String],
                _password: Option<&str>,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn extract_directory(
                &self,
                _path: &Path,
                _dest: &Path,
                _dir_path: &str,
                _password: Option<&str>,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn recompress_7z(&self, _source: &Path, _dest_7z: &Path) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn add_files(&self, _archive: &Path, _files: &[PathBuf]) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn create_archive(
                &self,
                _dest: &Path,
                _files: &[PathBuf],
                _format: &str,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn read_text_file(
                &self,
                _archive: &Path,
                _path_in_archive: &str,
                _password: Option<&str>,
            ) -> anyhow::Result<String> {
                unimplemented!()
            }
            fn delete_files(&self, _archive: &Path, _files: &[String]) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn add_or_update_file_from_str(
                &self,
                _archive: &Path,
                _path_in_archive: &str,
                _content: &str,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn convert_to_7z(
                &self,
                _source: &arclain_core::Archive,
                _dest: &Path,
                _temp_dir: &Path,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn crc32_of_entry(
                &self,
                _archive: &Path,
                _path_in_archive: &str,
                _password: Option<&str>,
            ) -> anyhow::Result<String> {
                unimplemented!()
            }
        }
        arclain_core::Archive::new(Arc::new(NoopBackend), PathBuf::from("dummy.zip"))
    }

    fn session_with(entries: &[arclain_core::ArchiveEntry]) -> ArchiveSession {
        ArchiveSession::new(
            ArchiveSessionId::from_raw(1),
            PathBuf::from("a.zip"),
            "zip".to_string(),
            dummy_archive(),
            entries,
        )
    }
}
