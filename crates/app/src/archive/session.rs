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
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::archive::{
    ArchiveEntryDto, ArchivePath, ArchiveSnapshot, EntryKind, EntryPage, EntrySortKey,
    ListEntriesRequest, SortDirection,
};
use crate::ids::{ArchiveSessionId, EntryId};

/// Session-scoped id assignment, keyed by `(entry kind, canonical path,
/// collision ordinal)` and carried across every [`EntryIndex::build`] a
/// session performs -- the initial open, and any future reindex after a
/// mutation (see the module doc comment). An unchanged path is handed
/// back the same [`EntryId`] every time, so a caller that cached an id
/// across a refresh (selection, expanded-folder state) keeps pointing at
/// the same logical entry for as long as that entry itself did not
/// change. A path new to this session mints a fresh id from the
/// per-session counter; a path that stops appearing simply stops being
/// handed out again -- nothing ever removes it from `assigned`, so its
/// id is never reassigned to a different path.
///
/// `kind` is part of the key, not just `(path, ordinal)`: a canonical
/// path can legitimately name both a real file *and* a directory this
/// session synthesizes as the parent of some deeper file (ordinary
/// content -- entries `foo` and `foo/bar` both present makes `ancestors`
/// synthesize a directory literally named `foo`). Both are assigned
/// under ordinal 0 for that identical path string; without `kind` in the
/// key they would collide onto the same [`EntryId`], and the second
/// assignment (in practice always the directory, built after every file)
/// would silently overwrite the file's row in `EntryIndex::by_id` --
/// breaking this module's own documented "duplicate paths preserved as
/// distinct entries" invariant. Keying on kind too keeps the file and the
/// directory permanently distinct, exactly like any other pair of
/// entries that happen to share a canonical path.
///
/// The "collision ordinal" is which occurrence (0, 1, 2, ...) of a
/// duplicate path *of the same kind* this is, in stable, deterministic
/// encounter order (see `EntryIndex::build`); every non-duplicate path is
/// always ordinal 0.
///
/// Ids are unique only within the owning session (see the module doc
/// comment): two different sessions may (and eventually will) hand out
/// the same raw id to unrelated entries. Every consumer already addresses
/// entries as a `(session, entry)` pair, never a bare [`EntryId`], so
/// this is not a correctness gap.
#[derive(Debug, Default)]
pub(crate) struct EntryIdAssigner {
    assigned: HashMap<(EntryKind, String, u64), EntryId>,
    next: u64,
}

impl EntryIdAssigner {
    /// Returns the id already assigned to `(kind, path, ordinal)`, or
    /// mints and remembers a fresh one the first time this key is seen.
    fn assign(&mut self, kind: EntryKind, path: &str, ordinal: u64) -> EntryId {
        let key = (kind, path.to_string(), ordinal);
        if let Some(id) = self.assigned.get(&key) {
            return *id;
        }
        self.next += 1;
        let id = EntryId::from_raw(self.next);
        self.assigned.insert(key, id);
        id
    }
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

/// Reproduces the pre-facade UI's `SortColumn::Crc32` key (see
/// `crates/ui/src/shared/models/file_entry.rs::sort_entry_indices`):
/// `crc32.to_uppercase()`, a plain string compare. The old `FileEntry::
/// crc32` was a non-optional `String`, empty for an entry with no CRC
/// (uppercasing an empty string is still an empty string); this facade's
/// [`ArchiveEntryDto::crc32`] is `Option<String>` for the same case, so
/// `None` is treated as that same empty string -- sorting before every
/// real (uppercased hex) CRC, ascending.
fn crc32_sort_key(dto: &ArchiveEntryDto) -> String {
    dto.crc32.as_deref().unwrap_or("").to_uppercase()
}

/// Reproduces the pre-facade UI's `SortColumn::Ratio` key
/// (`parse_ratio_pct` over a pre-formatted `"{pct}%"` display string).
/// The display string itself was always computed as `packed_size * 100 /
/// size` (truncating integer division) when `size > 0`, else the literal
/// `"0%"` -- see `crates/ui/src/core/utils.rs::convert_to_file_entry`.
/// Recomputed directly from the already-numeric [`ArchiveEntryDto`]
/// fields here rather than round-tripped through a display string, which
/// is a pure representation change (same reasoning as
/// [`parse_modified_to_unix_ms`]'s doc comment): the zero-size fallback
/// (an empty file, or a folder aggregating zero total bytes) reproduces
/// the old code's divide-by-zero guard exactly, and `saturating_mul`
/// guards the multiply the old string-formatting code never had to
/// consider (its `format!` happened once at listing time over sizes that
/// fit a `u64` percentage without overflow in any real archive; kept here
/// purely as defensive arithmetic, not a reachable behavior difference).
fn ratio_sort_key(dto: &ArchiveEntryDto) -> u64 {
    if dto.uncompressed_size == 0 {
        return 0;
    }
    let compressed = dto.compressed_size.unwrap_or(0);
    compressed.saturating_mul(100) / dto.uncompressed_size
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
    /// Every `File`-kind entry's id, in the same stable, path-sorted
    /// order `build` already computes (`ordered_files`) -- collected
    /// alongside that existing pass, not a second scan. Backs
    /// [`Self::file_count`]/[`Self::file_paths_page`] with a `Vec` slice
    /// instead of `Self::file_paths`'s HashMap-order full materialization,
    /// so counting or paging never touches (let alone clones) an entry
    /// outside the requested page.
    sorted_file_ids: Vec<EntryId>,
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
    pub(crate) fn build(
        entries: &[arclain_core::ArchiveEntry],
        assigner: &mut EntryIdAssigner,
    ) -> Self {
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

        let mut by_id: HashMap<EntryId, ArchiveEntryDto> = HashMap::new();
        let mut children: HashMap<ArchivePath, Vec<EntryId>> = HashMap::new();
        let mut entry_count: u64 = 0;
        let mut total_uncompressed_size: u64 = 0;
        // Populated in the same order the `ordered_files` loop below
        // already walks (path-sorted) -- see `sorted_file_ids`'s own doc
        // comment for why this rides along with that existing pass
        // instead of a second scan.
        let mut sorted_file_ids: Vec<EntryId> = Vec::new();

        // Every directory's running aggregate, built by a single
        // ancestor-walk pass over the files below instead of a full
        // rescan per directory -- see `FolderTotals`'s own doc comment.
        let mut folder_totals: HashMap<String, FolderTotals> = HashMap::new();

        // Files: duplicates preserved. Sorting by path is a stable sort,
        // so entries sharing a path keep their encounter-order relative
        // position, which `ordinal_for_path` below turns into each
        // duplicate's "collision ordinal" -- the second component of the
        // key `EntryIdAssigner` mints/remembers each entry's id under
        // (see its own doc comment; the first component is the entry's
        // kind, so a file and a directory that happen to share a
        // canonical path -- see `EntryIdAssigner`'s doc comment -- never
        // collide onto the same ordinal-0 key).
        let mut ordered_files = files;
        ordered_files.sort_by(|a, b| a.0.cmp(&b.0));

        let mut ordinal_for_path: HashMap<String, u64> = HashMap::new();
        for (path, raw) in ordered_files {
            let ordinal = {
                let counter = ordinal_for_path.entry(path.clone()).or_insert(0);
                let this_ordinal = *counter;
                *counter += 1;
                this_ordinal
            };
            let id = assigner.assign(EntryKind::File, &path, ordinal);
            let name = basename(&path).to_string();
            let parent = ArchivePath::parse(parent_of(&path).to_string())
                .unwrap_or_else(|_| ArchivePath::root());
            entry_count += 1;
            total_uncompressed_size = total_uncompressed_size.saturating_add(raw.uncompressed_size);

            // Shared once per file rather than cloned into every ancestor
            // below: a file at depth N would otherwise heap-allocate and
            // copy its own path string N times (once per ancestor), which
            // for a large, deeply nested archive is real transient memory
            // (`O(files * depth * path_len)`). `Arc<str>::clone` is a
            // refcount bump, not a byte copy. Skipped entirely when there
            // is no crc32 to record, since that is this value's only use.
            let shared_path: Option<Arc<str>> =
                raw.crc32.is_some().then(|| Arc::from(path.as_str()));

            for ancestor in ancestors(&path) {
                let totals = folder_totals.entry(ancestor).or_default();
                totals.uncompressed_size = totals
                    .uncompressed_size
                    .saturating_add(raw.uncompressed_size);
                totals.compressed_size = totals.compressed_size.saturating_add(raw.compressed_size);
                if let (Some(crc), Some(shared_path)) = (&raw.crc32, &shared_path) {
                    totals
                        .crc_pairs
                        .push((shared_path.clone(), crc.to_uppercase()));
                }
            }

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
            sorted_file_ids.push(id);
        }

        for (path, raw) in dirs {
            let totals = folder_totals.remove(&path).unwrap_or_default();
            let id = assigner.assign(EntryKind::Directory, &path, 0);
            let name = basename(&path).to_string();
            let parent = ArchivePath::parse(parent_of(&path).to_string())
                .unwrap_or_else(|_| ArchivePath::root());
            entry_count += 1;
            let dto = ArchiveEntryDto {
                id,
                path: ArchivePath::parse(path).unwrap_or_else(|_| ArchivePath::root()),
                name,
                kind: EntryKind::Directory,
                compressed_size: Some(totals.compressed_size),
                uncompressed_size: totals.uncompressed_size,
                modified_at_unix_ms: raw.modified.as_deref().and_then(parse_modified_to_unix_ms),
                encrypted: raw.encrypted,
                crc32: totals.finalize_crc(),
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
            sorted_file_ids,
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

    /// The archive-relative path and kind of one entry, or `None` if
    /// `entry_id` was never minted into this revision's index (a stale id
    /// from a superseded revision, or one a caller fabricated).
    fn get(&self, entry_id: EntryId) -> Option<&ArchiveEntryDto> {
        self.by_id.get(&entry_id)
    }

    /// Every indexed entry rebuilt into the flat
    /// `arclain_core::ArchiveEntry` shape the organization rule engine
    /// consumes, path-sorted so a caller feeding it to that engine gets
    /// a deterministic input regardless of this index's `HashMap`
    /// iteration order.
    ///
    /// Faithful for exactly the three fields that engine reads -- `path`,
    /// `is_dir`, and (for the 0-byte prune) `size`. The rest are carried
    /// through as the index holds them, which differs from a raw backend
    /// listing in two documented ways, neither of which the engine can
    /// observe: a directory's `size`/`packed_size` are this index's
    /// recursive aggregates rather than the row's own value, and
    /// `modified` is always `None` (the index stores a parsed Unix
    /// timestamp, and reconstituting the backend's original date string
    /// from it would be inventing bytes nothing reads).
    ///
    /// The directory set is likewise the index's: every ancestor a file
    /// path implies is present, whether or not the archive listed it
    /// explicitly. That is invisible to the plan -- `prune_entries`
    /// flattens to files only, so no directory entry ever reaches move
    /// computation or content-root detection -- but it *is* what an
    /// integrity report's folder count sees, and it makes that count
    /// agree with the folder count `list_entries` reports for the same
    /// session.
    fn organization_entries(&self) -> Vec<arclain_core::ArchiveEntry> {
        let mut entries: Vec<arclain_core::ArchiveEntry> = self
            .by_id
            .values()
            .map(|dto| arclain_core::ArchiveEntry {
                path: dto.path.as_str().to_string(),
                size: dto.uncompressed_size,
                packed_size: dto.compressed_size.unwrap_or(0),
                modified: None,
                is_dir: matches!(dto.kind, EntryKind::Directory),
                encrypted: dto.encrypted,
                crc32: dto.crc32.clone(),
            })
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.is_dir.cmp(&b.is_dir)));
        entries
    }

    /// Every `File` entry's archive-relative path, in the same stable,
    /// path-sorted order [`Self::file_paths_page`] returns. What a
    /// whole-archive extraction pre-scans for destination collisions --
    /// there is no `EntryId` selection to resolve in that case, only
    /// "every file this archive would write".
    ///
    /// Walks `sorted_file_ids` rather than `by_id.values()`: iterating a
    /// `HashMap` yields an arbitrary, run-to-run-varying order, so the
    /// full list and any page of it would disagree about which path comes
    /// first, and two runs over an unchanged archive would produce
    /// differently-ordered lists. Both consumers of this method (a
    /// collision pre-scan, and the plugin bridge's `archive_entries`) are
    /// better served by a deterministic order, and `sorted_file_ids` is
    /// already built during indexing at no extra cost.
    ///
    /// Deliberately **not** what [`Self::file_count`]/
    /// [`Self::file_paths_page`] are built on: this materializes and
    /// clones every file path in the index (`O(files)` always), which is
    /// the correct cost for "give me the complete list" but the wrong one
    /// for "how many are there" or "give me one page" -- see those
    /// methods' own doc comments. `#[cfg(test)]` counts calls so a test
    /// can assert paging/counting never reaches this.
    fn file_paths(&self) -> Vec<String> {
        #[cfg(test)]
        FILE_PATHS_CALLS.with(|calls| calls.set(calls.get() + 1));
        self.sorted_file_ids
            .iter()
            .filter_map(|id| self.by_id.get(id))
            .map(|dto| dto.path.as_str().to_string())
            .collect()
    }

    /// Number of `File`-kind entries, without materializing or cloning any
    /// path -- `sorted_file_ids.len()` is tracked incrementally at build
    /// time, so this is `O(1)`. Matches [`Self::file_paths`]'s scope
    /// (files only, not the synthesized directories `entry_count` also
    /// includes).
    pub(crate) fn file_count(&self) -> usize {
        self.sorted_file_ids.len()
    }

    /// Clones only the requested page of file paths, in the same stable,
    /// path-sorted order [`Self::file_paths`] does not guarantee --
    /// `O(limit)` beyond an `O(1)` slice to `offset`, never touching (let
    /// alone cloning) a path outside the requested page.
    pub(crate) fn file_paths_page(&self, offset: usize, limit: usize) -> Vec<String> {
        self.sorted_file_ids
            .get(offset..)
            .unwrap_or(&[])
            .iter()
            .take(limit)
            .filter_map(|id| self.by_id.get(id))
            .map(|dto| dto.path.as_str().to_string())
            .collect()
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only instrumentation for [`EntryIndex::file_paths`] -- see
    /// that method's own doc comment.
    static FILE_PATHS_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One folder's running aggregate across every descendant file at any
/// depth. Built by a single ancestor-walk pass over every file in
/// [`EntryIndex::build`] -- each file adds itself to every one of its
/// ancestors' `FolderTotals` exactly once, in `O(files * depth)` total --
/// instead of the old `aggregate_folder`'s full rescan of every indexed
/// entry, once per directory (`O(directories * files)`, and quadratic in
/// the common case where a listing's directory count scales with its
/// file count). `Default` gives every folder, including one synthesized
/// purely from an implied ancestor with no direct file of its own, a
/// correct all-zero starting point.
///
/// One narrow, deliberate behavior difference from the old per-directory
/// rescan: the old code also matched a file whose path was *exactly
/// equal* to the folder's own path (`path == folder_path`), which could
/// only ever fire for a malformed archive reporting both a file and a
/// directory entry at the identical canonical path. A proper-ancestors
/// walk (see `ancestors`) never includes a path in its own ancestor list,
/// so that self-match no longer contributes to the colliding directory's
/// totals here. Not reproduced: it has no sensible interpretation (a
/// file is not its own descendant) and cannot arise from any archive this
/// workspace's backends produce in practice.
#[derive(Default)]
struct FolderTotals {
    uncompressed_size: u64,
    compressed_size: u64,
    /// `(file path, uppercased crc32)` pairs, one per descendant file
    /// that reported a crc32, accumulated in file-processing order. The
    /// path is `Arc<str>` rather than `String`: a file at depth N is
    /// pushed into N ancestors' totals, and the caller hands us the same
    /// shared `Arc` for every one of those pushes (see its construction
    /// in `EntryIndex::build`) rather than a fresh clone per ancestor.
    crc_pairs: Vec<(Arc<str>, String)>,
}

impl FolderTotals {
    /// Combines every accumulated `(path, crc)` pair into one folder-level
    /// CRC-32, sorting by path first so the combined digest is
    /// independent of backend listing order. Mirrors the old
    /// `aggregate_folder`'s exact hash construction (and
    /// `NavigationState::compute_folder_crc`'s before it). Comparing and
    /// hashing `Arc<str>` compares/hashes the referenced string content
    /// (via `Deref`), not the pointer, so this is byte-for-byte identical
    /// to the `String`-keyed version it replaces.
    fn finalize_crc(mut self) -> Option<String> {
        if self.crc_pairs.is_empty() {
            return None;
        }
        self.crc_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut hasher = crc32fast::Hasher::new();
        for (path, crc) in &self.crc_pairs {
            hasher.update(path.as_bytes());
            hasher.update(b":");
            hasher.update(crc.as_bytes());
            hasher.update(b"\n");
        }
        Some(format!("{:08X}", hasher.finalize()))
    }
}

/// One open archive: its backend handle (for future read/mutate
/// operations to reuse), its indexed entries, and the metadata a
/// [`ArchiveSnapshot`] reports.
#[derive(Debug)]
pub(crate) struct ArchiveSession {
    id: ArchiveSessionId,
    /// The archive's on-disk path. A plain `RwLock`, not a bare `PathBuf`:
    /// [`Self::set_source_path`] lets `ArchiveContextBridge::set_archive_path`
    /// (the `rename_archive` host function's write sink) update it in
    /// place, matching the pre-facade UI bridge's own "plain unconditional
    /// set, no revision bump, no re-list" behavior for the same call.
    source_path: RwLock<PathBuf>,
    /// Metadata a plugin has reported for this session via
    /// `ArchiveContextBridge::set_session_metadata`/`set_active_tab_metadata`
    /// (the `emit_metadata` host function's write sink). `None` until a
    /// plugin actually reports something; also what [`Self::snapshot`]
    /// reports as `ArchiveSnapshot::metadata`.
    metadata: RwLock<Option<serde_json::Value>>,
    archive_type: String,
    archive: Arc<Mutex<arclain_core::Archive>>,
    revision: AtomicU64,
    entry_index: RwLock<EntryIndex>,
    /// Persists id assignment across every `EntryIndex::build` this
    /// session performs -- see [`EntryIdAssigner`]'s own doc comment.
    id_assigner: Mutex<EntryIdAssigner>,
    /// Serializes `crate::operations::archive_mutation`'s "check
    /// `expected_revision`, mutate the backend, re-list, rebuild the
    /// index, bump `revision`" sequence into one atomic unit *per this
    /// session*. Two concurrent mutation operations that both hold a
    /// reference to *this* `ArchiveSession` value cannot race each
    /// other -- whichever acquires this lock first runs its whole
    /// sequence, including the reindex, before the second one even reads
    /// `revision` -- which is what makes `expected_revision` race-free
    /// rather than merely sequential for that case.
    ///
    /// That scope is real and narrower than "this archive file cannot be
    /// concurrently rewritten twice": the lock lives on the `ArchiveSession`
    /// value itself, not on the underlying `source_path`. Closing this
    /// session and reopening the same path -- or a second tab
    /// independently opening the identical path -- mints a *different*
    /// `ArchiveSession` with its own, fresh `mutation_lock`, which knows
    /// nothing about this one. Two such sessions over the same physical
    /// file can therefore still race each other's whole-archive
    /// extract-modify-recompress rewrites, exactly as a concurrent
    /// extraction reading that same file mid-rewrite already could
    /// pre-facade -- this is a pre-existing weakness this task does not
    /// attempt to close; a real fix needs serialization keyed on the
    /// resolved path itself (e.g. a process-wide `Mutex` map in
    /// `ArchiveSessionStore`), which is a larger change than this lock's
    /// job of making one session's own optimistic-concurrency check
    /// trustworthy.
    ///
    /// An async `tokio::sync::Mutex`, not `parking_lot`/`std`: the
    /// critical section it guards spans real blocking backend I/O
    /// awaited via `spawn_blocking`, and only an async mutex can be held
    /// across an `.await` point without blocking a worker thread while
    /// contended. Deliberately a *different* lock from `archive` above
    /// (which stays a quick, uncontended `parking_lot` peek at the
    /// backend/password, exactly as extraction already uses it) --
    /// holding `archive`'s own lock for a whole mutation's duration
    /// would make a concurrent extraction's brief password peek block a
    /// live async worker thread for as long as the mutation runs.
    mutation_lock: tokio::sync::Mutex<()>,
    /// Set once a mutation's own backend call has already succeeded but
    /// the follow-up re-list needed to safely reindex has failed (see
    /// `crate::operations::archive_mutation`'s relist-failure handling).
    /// Once true, every subsequent mutation attempt on this session is
    /// rejected outright, regardless of its own `expected_revision` --
    /// `revision` itself is *also* bumped when this is set (see
    /// [`Self::mark_desynced`]), but that alone is not sufficient: a
    /// caller who happens to read the bumped revision back (via a plain
    /// `list_entries`/`archive_snapshot` call, which does not consult this
    /// flag) would otherwise see a `expected_revision` that looks
    /// perfectly current, resolved against an index this session can no
    /// longer prove is correct. Never cleared within this session's own
    /// lifetime: nothing here can rebuild a trustworthy index from a
    /// known-stale one without a fresh backend listing, so the only real
    /// recovery is closing this session and opening a new one (which
    /// starts with this `false` by construction, from a real listing).
    desynced: AtomicBool,
}

impl ArchiveSession {
    pub(crate) fn new(
        id: ArchiveSessionId,
        source_path: PathBuf,
        archive_type: String,
        archive: arclain_core::Archive,
        entries: &[arclain_core::ArchiveEntry],
    ) -> Self {
        let mut id_assigner = EntryIdAssigner::default();
        let entry_index = EntryIndex::build(entries, &mut id_assigner);
        Self {
            id,
            source_path: RwLock::new(source_path),
            metadata: RwLock::new(None),
            archive_type,
            archive: Arc::new(Mutex::new(archive)),
            revision: AtomicU64::new(1),
            entry_index: RwLock::new(entry_index),
            id_assigner: Mutex::new(id_assigner),
            mutation_lock: tokio::sync::Mutex::new(()),
            desynced: AtomicBool::new(false),
        }
    }

    pub(crate) fn id(&self) -> ArchiveSessionId {
        self.id
    }

    /// Overwrites this session's on-disk path in place -- the
    /// `rename_archive` host function's write sink, via
    /// `ArchiveContextBridge::set_archive_path`. A plain, unconditional
    /// write: no revision bump, no re-list, matching the pre-facade UI
    /// bridge's own identical behavior for the same call.
    ///
    /// `#[allow(dead_code)]`: exercised by `crate::plugins`'s own unit
    /// tests (`ArchiveContextBridge` is implemented and tested standalone),
    /// but that bridge is not yet `PluginManager`'s installed one in
    /// production -- see `crate::plugins`'s own module doc comment for
    /// why, and what closes the gap.
    #[allow(dead_code)]
    pub(crate) fn set_source_path(&self, path: PathBuf) {
        *self.source_path.write() = path;
    }

    /// Overwrites this session's plugin-reported metadata -- the
    /// `emit_metadata` host function's write sink, via
    /// `ArchiveContextBridge::set_session_metadata`/`set_active_tab_metadata`.
    /// `#[allow(dead_code)]` for the same reason as [`Self::set_source_path`].
    #[allow(dead_code)]
    pub(crate) fn set_metadata(&self, metadata: Option<serde_json::Value>) {
        *self.metadata.write() = metadata;
    }

    /// Not called by anything this task adds: a future mutation-aware
    /// reindex rebuilds `entry_index` and needs this same assigner
    /// (locked for the rebuild's duration) so ids stay stable across it --
    /// see [`EntryIdAssigner`]'s own doc comment. Kept as the seam that
    /// future task reuses rather than re-deriving its own way to reach
    /// this session's id-assignment state.
    #[allow(dead_code)]
    pub(crate) fn id_assigner(&self) -> &Mutex<EntryIdAssigner> {
        &self.id_assigner
    }

    /// The lock `crate::operations::archive_mutation` holds across its
    /// whole "check revision, mutate, re-list, reindex, bump revision"
    /// sequence -- see [`Self`]'s own field doc comment for why this is a
    /// separate lock from the one guarding `archive`.
    pub(crate) fn mutation_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.mutation_lock
    }

    /// True once this session has been marked desynced -- see
    /// [`Self::mark_desynced`] and the `desynced` field's own doc
    /// comment. Checked by `crate::operations::archive_mutation` before
    /// every mutation attempt, independent of (and before) its own
    /// `expected_revision` comparison.
    pub(crate) fn is_desynced(&self) -> bool {
        self.desynced.load(Ordering::SeqCst)
    }

    /// Marks this session desynced and bumps `revision` -- called by
    /// `crate::operations::archive_mutation` only when a mutation's own
    /// backend call already succeeded but the follow-up re-list needed to
    /// safely reindex failed (see [`Self::reindex`]'s own contract: it is
    /// simply never called in that case, since there is no fresh entry
    /// list to build it from). Bumping `revision` here, on top of setting
    /// the flag, is deliberate belt-and-suspenders: it ensures a stale
    /// `expected_revision` a caller already held is rejected even if some
    /// future call path forgot to check [`Self::is_desynced`] directly,
    /// without relying on that omission never happening.
    pub(crate) fn mark_desynced(&self) {
        self.desynced.store(true, Ordering::SeqCst);
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    /// Rebuilds this session's entry index from a fresh backend listing
    /// and bumps `revision` by exactly one, returning the new value.
    /// Called by `crate::operations::archive_mutation` only after the
    /// backend call that produced `entries` has already succeeded --
    /// `revision` must never advance on the strength of a mutation this
    /// session cannot prove landed.
    ///
    /// Reuses the *same* [`EntryIdAssigner`] the initial [`Self::new`]
    /// build used (see that type's own doc comment): a path unaffected by
    /// the mutation keeps the exact [`EntryId`] it already had, a
    /// genuinely new path mints a fresh one, and a path that stopped
    /// appearing simply stops being handed out again. This is what lets a
    /// caller's cached selection survive a mutation for every entry it
    /// did not touch.
    ///
    /// CPU-bound (walks every entry, synthesizes ancestor directories,
    /// aggregates folder totals) -- callers must invoke this from a
    /// blocking-safe context (`spawn_blocking`), matching
    /// `ArchiveSessionStore::open`'s identical requirement for the
    /// initial build.
    ///
    /// Ordering is load-bearing: the index write below happens-before
    /// the `AcqRel` revision bump in this thread's program order, so any
    /// caller that later observes the *new* revision via `Self::revision`'s
    /// `Acquire` load is guaranteed to also observe this index update --
    /// there is no window where a reader could see the bumped revision
    /// paired with the stale index.
    pub(crate) fn reindex(&self, entries: &[arclain_core::ArchiveEntry]) -> u64 {
        let mut assigner = self.id_assigner.lock();
        let new_index = EntryIndex::build(entries, &mut assigner);
        *self.entry_index.write() = new_index;
        self.revision.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Looks up one entry by id against this session's *current* index.
    /// `None` if `entry_id` was never minted into this revision's index --
    /// a stale id from a superseded revision, or one a caller fabricated.
    ///
    /// Used by `crate::operations::archive_mutation`'s `ReplaceText`
    /// handling to resolve the entry's current path and confirm it names
    /// a `File` (rather than a `Directory`) before ever calling the
    /// backend, and by the materialization operation
    /// (`crate::materialization`) to decide whether a requested entry is
    /// a single file (materializes to that one extracted path) or a
    /// directory (materializes to the whole extracted subtree, via
    /// [`Self::resolve_extractable_paths`]) -- the two cases a lease's
    /// `local_path` must distinguish.
    pub(crate) fn entry(&self, entry_id: EntryId) -> Option<ArchiveEntryDto> {
        self.entry_index.read().get(entry_id).cloned()
    }

    /// This session's entries in the shape the organization rule engine
    /// consumes, paired with the revision they belong to -- see
    /// [`EntryIndex::organization_entries`] for exactly how faithful the
    /// reconstruction is.
    ///
    /// The revision is read while the index guard is held, so the pair
    /// is never optimistic: a concurrent [`Self::reindex`] cannot swap
    /// the index (and therefore cannot reach its own revision bump)
    /// while this read is in flight, and one that swapped just before
    /// this acquired the guard reports the *older* revision alongside
    /// the newer entries. That direction is the safe one -- a caller
    /// comparing this against `archive_snapshot` concludes "stale,
    /// recompute", never "current" about entries that are not.
    ///
    /// CPU-bound over the whole entry list; callers must invoke it from
    /// a blocking-safe context, the same requirement [`Self::reindex`]
    /// carries.
    pub(crate) fn organization_entries(&self) -> (u64, Vec<arclain_core::ArchiveEntry>) {
        let index = self.entry_index.read();
        let revision = self.revision();
        (revision, index.organization_entries())
    }

    /// The metadata a plugin last reported for this session, as the raw
    /// JSON value `emit_metadata` wrote. `None` until a plugin reports
    /// something. Same value [`Self::snapshot`] carries; a distinct
    /// accessor so a caller that needs only this does not clone an
    /// entire snapshot (and does not take the entry-index lock) to get it.
    pub(crate) fn metadata(&self) -> Option<serde_json::Value> {
        self.metadata.read().clone()
    }

    /// The archive's current source path, alongside the backend handle
    /// [`Self::archive_arc`] returns. Read by the extraction operation
    /// (`crate::operations::extract`) to resolve what to hand the CLI
    /// runner. Returns an owned clone rather than `&Path`: the path lives
    /// behind a lock (see [`Self::set_source_path`]), so nothing can
    /// return a reference into it that outlives the read guard.
    pub(crate) fn source_path(&self) -> PathBuf {
        self.source_path.read().clone()
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
    /// Read by the extraction operation (`crate::operations::extract`) to
    /// reuse the exact password the session was opened with, rather than
    /// re-deriving its own way to reach the backend a session already
    /// resolved.
    pub(crate) fn archive_arc(&self) -> Arc<Mutex<arclain_core::Archive>> {
        self.archive.clone()
    }

    pub(crate) fn snapshot(&self) -> ArchiveSnapshot {
        let index = self.entry_index.read();
        ArchiveSnapshot {
            session_id: self.id,
            revision: self.revision(),
            source_path: self.source_path.read().clone(),
            archive_type: self.archive_type.clone(),
            entry_count: index.entry_count(),
            total_uncompressed_size: index.total_uncompressed_size(),
            // Neither `arclain_core::archive::ArchiveInfo` nor any backend
            // in this workspace reports an archive comment today; always
            // `None` until a future task adds a real source for it.
            comment: None,
            metadata: self.metadata.read().clone(),
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
            EntrySortKey::Compressed => {
                matching.sort_by_key(|dto| dto.compressed_size);
            }
            EntrySortKey::Crc32 => {
                matching.sort_by_key(|dto| crc32_sort_key(dto));
            }
            EntrySortKey::Encrypted => {
                matching.sort_by_key(|dto| dto.encrypted as u8);
            }
            EntrySortKey::Kind => {
                matching.sort_by_key(|dto| kind_sort_key(dto));
            }
            EntrySortKey::Modified => {
                matching.sort_by_key(|dto| dto.modified_at_unix_ms);
            }
            EntrySortKey::Name => {
                matching.sort_by_key(|dto| dto.name.to_lowercase());
            }
            EntrySortKey::Ratio => {
                matching.sort_by_key(|dto| ratio_sort_key(dto));
            }
            EntrySortKey::Size => {
                matching.sort_by_key(|dto| dto.uncompressed_size);
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

    /// Resolves `entry_ids` to the concrete archive-relative file paths
    /// an extraction hands to the CLI backend: a `File` (or `Symlink`,
    /// see [`kind_sort_key`]'s own comment on why that variant is treated
    /// like `File`) entry resolves to its own path; a `Directory` entry
    /// expands to every descendant file at any depth, matching
    /// [`EntryIndex::build`]'s own recursive-prefix convention (see
    /// [`aggregate_folder`]). Paths reached through more than one
    /// requested id (a file requested directly AND via an ancestor
    /// directory also in `entry_ids`) are deduplicated.
    ///
    /// Every returned path already passed [`ArchivePath::parse`] at index-
    /// build time (never absolute, never containing a `..` segment) --
    /// [`EntryIndex::build`] skips any entry that fails that check, so it
    /// never receives an [`EntryId`] in the first place. A caller can
    /// therefore never smuggle a path-traversal string into an
    /// extraction's file list merely by selecting entries through this
    /// method.
    ///
    /// Returns the first entry id not present in this session's current
    /// index, if any -- a stale id from a superseded revision, or one the
    /// caller fabricated.
    pub(crate) fn resolve_extractable_paths(
        &self,
        entry_ids: &[EntryId],
    ) -> Result<Vec<String>, EntryId> {
        let index = self.entry_index.read();
        let mut seen = std::collections::HashSet::new();
        let mut paths = Vec::new();
        for &id in entry_ids {
            let dto = index.get(id).ok_or(id)?;
            match dto.kind {
                EntryKind::File | EntryKind::Symlink => {
                    if seen.insert(dto.path.as_str().to_string()) {
                        paths.push(dto.path.as_str().to_string());
                    }
                }
                EntryKind::Directory => {
                    let prefix = format!("{}/", dto.path.as_str());
                    for candidate in index.by_id.values() {
                        if candidate.kind == EntryKind::File
                            && candidate.path.as_str().starts_with(&prefix)
                            && seen.insert(candidate.path.as_str().to_string())
                        {
                            paths.push(candidate.path.as_str().to_string());
                        }
                    }
                }
            }
        }
        Ok(paths)
    }

    /// Every `File` entry's archive-relative path in this session's
    /// current index. What a whole-archive extraction pre-scans for
    /// destination collisions -- there is no `EntryId` selection to
    /// resolve in that case, only "every file this archive would write".
    pub(crate) fn all_file_paths(&self) -> Vec<String> {
        self.entry_index.read().file_paths()
    }

    /// Number of `File`-kind entries in this session's current index,
    /// without materializing any path -- see
    /// [`EntryIndex::file_count`]'s own doc comment.
    pub(crate) fn file_count(&self) -> usize {
        self.entry_index.read().file_count()
    }

    /// One page of file paths from this session's current index, without
    /// materializing the rest -- see [`EntryIndex::file_paths_page`]'s
    /// own doc comment.
    pub(crate) fn file_paths_page(&self, offset: usize, limit: usize) -> Vec<String> {
        self.entry_index.read().file_paths_page(offset, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    fn file_with_crc32(path: &str, crc32: &str) -> arclain_core::ArchiveEntry {
        arclain_core::ArchiveEntry {
            path: path.to_string(),
            size: 1,
            packed_size: 1,
            modified: None,
            is_dir: false,
            encrypted: false,
            crc32: Some(crc32.to_string()),
        }
    }

    fn encrypted_file(path: &str) -> arclain_core::ArchiveEntry {
        arclain_core::ArchiveEntry {
            path: path.to_string(),
            size: 1,
            packed_size: 1,
            modified: None,
            is_dir: false,
            encrypted: true,
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
        let session = session_with(&entries);
        let page = session.list_entries(&request(""));

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
    fn folder_aggregation_accumulates_correctly_at_every_nesting_level() {
        // Guards the ancestor-walk accumulation pass (see `FolderTotals`)
        // against a regression to the old per-directory full rescan it
        // replaced: three levels deep, with a sibling file at the
        // shallowest level, so a bug that aggregated only one level, or
        // leaked a sibling's size into an unrelated folder, would be
        // caught. `a` must include every descendant at any depth
        // (including `a/other.txt`); `a/b` must include its own
        // descendants but never `a`'s sibling file.
        let entries = vec![
            file("a/other.txt", 1, 1),
            file("a/b/one.txt", 10, 5),
            file("a/b/c/two.txt", 100, 50),
        ];
        let session = session_with(&entries);

        let root_page = session.list_entries(&request(""));
        let a = root_page.entries.iter().find(|e| e.name == "a").unwrap();
        assert_eq!(a.uncompressed_size, 1 + 10 + 100);
        assert_eq!(a.compressed_size, Some(1 + 5 + 50));

        let a_page = session.list_entries(&request("a"));
        let b = a_page.entries.iter().find(|e| e.name == "b").unwrap();
        assert_eq!(
            b.uncompressed_size,
            10 + 100,
            "must not include a's sibling other.txt"
        );
        assert_eq!(b.compressed_size, Some(5 + 50));
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
    fn a_path_that_is_both_a_real_file_and_a_synthesized_directory_prefix_keeps_two_distinct_entries(
    ) {
        // "foo" is listed as an explicit file AND, because "foo/bar" also
        // exists, implied as the parent directory of "foo/bar" -- an
        // ordinary (if unusual) archive shape, not an adversarial one.
        // Both must survive as distinct rows: the id assigner's key must
        // include entry kind, or the file and the synthesized directory
        // (both minted under ordinal 0 for the identical path "foo")
        // collide onto the same EntryId, and the second one built (always
        // the directory, since directories are processed after files)
        // silently overwrites the file's row in `by_id`.
        let entries = vec![file("foo", 1, 1), file("foo/bar", 2, 2)];
        let session = session_with(&entries);

        let root_page = session.list_entries(&request(""));
        let foo_rows: Vec<_> = root_page
            .entries
            .iter()
            .filter(|e| e.name == "foo")
            .collect();
        assert_eq!(
            foo_rows.len(),
            2,
            "both the file 'foo' and the synthesized directory 'foo' must appear as separate rows"
        );
        assert_eq!(
            root_page.total, 2,
            "the root listing must show both rows exactly once each"
        );
        assert_ne!(
            foo_rows[0].id, foo_rows[1].id,
            "a file and a directory at the same canonical path must never collide onto one id"
        );
        assert!(foo_rows.iter().any(|e| e.kind == EntryKind::File));
        assert!(foo_rows.iter().any(|e| e.kind == EntryKind::Directory));

        // Both ids must actually resolve in `by_id`, not just appear once
        // and silently share a row with the other's content overwritten.
        let file_row = foo_rows.iter().find(|e| e.kind == EntryKind::File).unwrap();
        let dir_row = foo_rows
            .iter()
            .find(|e| e.kind == EntryKind::Directory)
            .unwrap();
        assert_eq!(
            file_row.uncompressed_size, 1,
            "the file's own size, not overwritten"
        );
        assert_eq!(
            dir_row.uncompressed_size, 2,
            "the directory's aggregate (foo/bar's size), not the file's"
        );
    }

    #[test]
    fn entry_ids_are_stable_across_repeated_queries_and_independent_of_sort_key() {
        let entries = vec![file("a.txt", 1, 1), file("b.txt", 2, 2)];
        let session = session_with(&entries);

        let first = session.list_entries(&request(""));
        let second = session.list_entries(&request(""));
        assert_eq!(first.entries[0].id, second.entries[0].id);
        assert_eq!(first.entries[1].id, second.entries[1].id);

        // A same-params re-list alone cannot catch id assignment that
        // accidentally depended on iteration/sort order: an unchanged
        // order would coincidentally reproduce the same ids even if the
        // underlying minting were order-dependent. A different sort key
        // AND direction reorders the page but must never re-mint or swap
        // ids for the same underlying entries.
        let mut by_size_desc = request("");
        by_size_desc.sort_key = EntrySortKey::Size;
        by_size_desc.sort_direction = SortDirection::Descending;
        let reordered = session.list_entries(&by_size_desc);

        let id_of = |page: &EntryPage, name: &str| {
            page.entries
                .iter()
                .find(|entry| entry.name == name)
                .unwrap()
                .id
        };
        assert_eq!(id_of(&first, "a.txt"), id_of(&reordered, "a.txt"));
        assert_eq!(id_of(&first, "b.txt"), id_of(&reordered, "b.txt"));
    }

    /// Finds an entry's id by its indexed path, without going through a
    /// `list_entries` page (whose directory scoping and sort/filter would
    /// be irrelevant noise for the rebuild-stability tests below, which
    /// care only about identity, not display order).
    fn id_for_path(index: &EntryIndex, path: &str) -> EntryId {
        index
            .by_id
            .values()
            .find(|dto| dto.path.as_str() == path)
            .map(|dto| dto.id)
            .unwrap_or_else(|| panic!("path {path:?} must exist in the index"))
    }

    #[test]
    fn rebuilding_with_identical_content_preserves_every_id() {
        let entries = vec![file("a.txt", 1, 1), file("dir/b.txt", 2, 2)];
        let mut assigner = EntryIdAssigner::default();
        let first = EntryIndex::build(&entries, &mut assigner);
        let second = EntryIndex::build(&entries, &mut assigner);

        assert_eq!(id_for_path(&first, "a.txt"), id_for_path(&second, "a.txt"));
        assert_eq!(
            id_for_path(&first, "dir/b.txt"),
            id_for_path(&second, "dir/b.txt")
        );
        assert_eq!(
            id_for_path(&first, "dir"),
            id_for_path(&second, "dir"),
            "a synthesized folder's id must also be stable across an identical rebuild"
        );
    }

    #[test]
    fn rebuild_after_a_simulated_mutation_preserves_unchanged_ids_mints_new_ones_and_never_reuses_a_removed_id(
    ) {
        let before = vec![file("a.txt", 1, 1), file("b.txt", 2, 2)];
        let mut assigner = EntryIdAssigner::default();
        let first = EntryIndex::build(&before, &mut assigner);
        let a_id_before = id_for_path(&first, "a.txt");
        let b_id = id_for_path(&first, "b.txt");

        // Simulates a mutation between two rebuilds of the SAME session:
        // "b.txt" is removed, "c.txt" is newly added, "a.txt" is untouched.
        let after = vec![file("a.txt", 1, 1), file("c.txt", 3, 3)];
        let second = EntryIndex::build(&after, &mut assigner);
        let a_id_after = id_for_path(&second, "a.txt");
        let c_id = id_for_path(&second, "c.txt");

        assert_eq!(
            a_id_before, a_id_after,
            "an unchanged path keeps its id across a reindex"
        );
        assert_ne!(
            c_id, a_id_before,
            "a path new to this session mints a fresh id"
        );
        assert_ne!(
            c_id, b_id,
            "a removed path's id is never reused for a different path"
        );
        assert!(
            second
                .by_id
                .values()
                .all(|dto| dto.path.as_str() != "b.txt"),
            "the removed path is no longer present in the rebuilt index"
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
    fn sort_by_compressed_orders_by_exact_compressed_bytes_not_uncompressed_size() {
        // Deliberately opposite orderings for compressed vs. uncompressed
        // size, so a test that accidentally sorted by the wrong field
        // would fail instead of coincidentally passing.
        let entries = vec![
            file("small_but_incompressible.bin", 10, 8),
            file("large_but_compressible.bin", 1000, 2),
        ];
        let session = session_with(&entries);

        let mut req = request("");
        req.sort_key = EntrySortKey::Compressed;
        let page = session.list_entries(&req);
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["large_but_compressible.bin", "small_but_incompressible.bin"]
        );
    }

    #[test]
    fn sort_by_crc32_is_uppercased_and_a_missing_crc_sorts_first() {
        let entries = vec![
            file_with_crc32("has_crc.bin", "abcd"),
            file("no_crc.bin", 1, 1),
        ];
        let session = session_with(&entries);

        let mut req = request("");
        req.sort_key = EntrySortKey::Crc32;
        let page = session.list_entries(&req);
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["no_crc.bin", "has_crc.bin"],
            "a missing crc32 sorts as an empty string, before any real (uppercased) crc"
        );
    }

    #[test]
    fn sort_by_encrypted_orders_unencrypted_before_encrypted_ascending() {
        let entries = vec![encrypted_file("locked.bin"), file("plain.bin", 1, 1)];
        let session = session_with(&entries);

        let mut req = request("");
        req.sort_key = EntrySortKey::Encrypted;
        let page = session.list_entries(&req);
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["plain.bin", "locked.bin"]);
    }

    #[test]
    fn sort_by_ratio_computes_truncating_percentage_and_treats_zero_size_as_zero() {
        let entries = vec![
            file("half.bin", 100, 50),  // 50%
            file("empty.bin", 0, 0),    // zero-size fallback: 0%, not a divide-by-zero panic
            file("full.bin", 100, 100), // 100%
        ];
        let session = session_with(&entries);

        let mut req = request("");
        req.sort_key = EntrySortKey::Ratio;
        let page = session.list_entries(&req);
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["empty.bin", "half.bin", "full.bin"]);
    }

    #[test]
    fn sort_ties_preserve_the_build_times_alphabetical_order() {
        // Both entries share the same compressed size, so this exercises
        // the stable-sort tie-break: `children` is pre-sorted by name at
        // build time (see `EntryIndex`'s own doc comment), and
        // `list_entries`'s own sort is stable, so entries tied on the
        // chosen key must keep that alphabetical order.
        let entries = vec![file("zeta.txt", 1, 5), file("alpha.txt", 1, 5)];
        let session = session_with(&entries);

        let mut req = request("");
        req.sort_key = EntrySortKey::Compressed;
        let page = session.list_entries(&req);
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["alpha.txt", "zeta.txt"],
            "entries tied on Compressed keep their build-time alphabetical order"
        );
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
    fn resolve_extractable_paths_expands_a_directory_id_to_its_descendant_files() {
        let entries = vec![
            file("game/Game.exe", 1, 1),
            file("game/data/save.dat", 1, 1),
            file("readme.txt", 1, 1),
        ];
        let session = session_with(&entries);
        let root = session.list_entries(&request(""));
        let game_dir = root.entries.iter().find(|e| e.name == "game").unwrap();

        let resolved = session
            .resolve_extractable_paths(&[game_dir.id])
            .expect("a real directory id must resolve");

        let mut resolved = resolved;
        resolved.sort();
        assert_eq!(resolved, ["game/Game.exe", "game/data/save.dat"]);
    }

    #[test]
    fn resolve_extractable_paths_deduplicates_a_file_reached_two_ways() {
        let entries = vec![file("game/Game.exe", 1, 1)];
        let session = session_with(&entries);
        let root = session.list_entries(&request(""));
        let game_dir = root.entries.iter().find(|e| e.name == "game").unwrap();
        let nested = session.list_entries(&request("game"));
        let game_exe = nested
            .entries
            .iter()
            .find(|e| e.name == "Game.exe")
            .unwrap();

        // Requesting both the directory AND the file it already contains
        // must not extract the same file twice.
        let resolved = session
            .resolve_extractable_paths(&[game_dir.id, game_exe.id])
            .unwrap();
        assert_eq!(resolved, ["game/Game.exe"]);
    }

    #[test]
    fn resolve_extractable_paths_rejects_an_unknown_entry_id() {
        let entries = vec![file("a.txt", 1, 1)];
        let session = session_with(&entries);
        let bogus = EntryId::from_raw(999_999);

        let error = session.resolve_extractable_paths(&[bogus]).unwrap_err();
        assert_eq!(error, bogus);
    }

    #[test]
    fn all_file_paths_lists_every_file_but_no_synthesized_folders() {
        let entries = vec![file("a.txt", 1, 1), file("dir/b.txt", 1, 1)];
        let session = session_with(&entries);

        let mut paths = session.all_file_paths();
        paths.sort();
        assert_eq!(paths, ["a.txt", "dir/b.txt"]);
    }

    /// Regression test: `file_count`/`file_paths_page` must never fall
    /// back to `file_paths`'s own full materialization (`ActiveTabBridge::
    /// archive_entry_count`/`archive_entries_page`'s trait-default
    /// behavior before `ArchiveContextBridge` overrode them) -- correct
    /// paging behavior alone (first/last/past-the-end/full-walk) would
    /// look identical whether or not the implementation wastefully
    /// materialized everything first, so this also asserts the call count
    /// on the expensive path stays at zero throughout.
    #[test]
    fn file_count_and_file_paths_page_do_not_materialize_the_full_file_list() {
        const TOTAL: usize = 5_000;
        let entries: Vec<_> = (0..TOTAL)
            .map(|i| file(&format!("file_{i:05}.bin"), 1, 1))
            .collect();
        let session = session_with(&entries);

        // Reset in case an earlier test running on this same OS thread
        // already called `file_paths` -- the counter is `thread_local`,
        // not per-test.
        FILE_PATHS_CALLS.with(|calls| calls.set(0));

        assert_eq!(session.file_count(), TOTAL);

        let first_page = session.file_paths_page(0, 10);
        assert_eq!(first_page.len(), 10);
        assert_eq!(first_page[0], "file_00000.bin");
        assert_eq!(first_page[9], "file_00009.bin");

        let last_page = session.file_paths_page(TOTAL - 3, 10);
        assert_eq!(
            last_page,
            vec!["file_04997.bin", "file_04998.bin", "file_04999.bin"],
            "a page whose offset is near the end must return only what's left, not pad or wrap"
        );

        assert!(
            session.file_paths_page(TOTAL + 100, 10).is_empty(),
            "an offset past the end must return an empty page, not panic"
        );

        // Walk every page and confirm the whole archive is covered
        // exactly once, with a stable order across calls.
        let mut seen = std::collections::HashSet::new();
        let mut offset = 0;
        loop {
            let page = session.file_paths_page(offset, 137);
            if page.is_empty() {
                break;
            }
            for path in &page {
                assert!(
                    seen.insert(path.clone()),
                    "duplicate path across pages: {path}"
                );
            }
            offset += page.len();
        }
        assert_eq!(seen.len(), TOTAL);

        assert_eq!(
            FILE_PATHS_CALLS.with(|calls| calls.get()),
            0,
            "file_count/file_paths_page must never fall back to the full-materialization \
             file_paths"
        );
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

    // ========================================================================
    // organization_entries: the shape the rule engine consumes.
    // ========================================================================

    /// The three fields `RuleEngine`/`IntegrityReport` actually read --
    /// `path`, `is_dir`, `size` -- must come back exactly as the backend
    /// listed them, in a deterministic order.
    #[test]
    fn organization_entries_reproduce_every_file_the_backend_listed() {
        let session = session_with(&[
            file("wrapper/b.bin", 20, 5),
            file("wrapper/a.exe", 10, 3),
            file("wrapper/empty.log", 0, 0),
        ]);

        let (revision, entries) = session.organization_entries();
        assert_eq!(revision, 1);

        let files: Vec<(&str, u64)> = entries
            .iter()
            .filter(|entry| !entry.is_dir)
            .map(|entry| (entry.path.as_str(), entry.size))
            .collect();
        assert_eq!(
            files,
            vec![
                ("wrapper/a.exe", 10),
                ("wrapper/b.bin", 20),
                ("wrapper/empty.log", 0),
            ],
            "files come back path-sorted, with their own sizes intact"
        );
    }

    /// The index synthesizes an ancestor directory the archive never
    /// listed. That is invisible to a plan (`prune_entries` flattens to
    /// files only) but it *is* what an integrity report's folder count
    /// sees, so pin it rather than leaving it to be discovered.
    #[test]
    fn organization_entries_include_directories_the_index_synthesized() {
        let session = session_with(&[file("wrapper/nested/deep.bin", 1, 1)]);

        let (_, entries) = session.organization_entries();
        let directories: Vec<&str> = entries
            .iter()
            .filter(|entry| entry.is_dir)
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(directories, vec!["wrapper", "wrapper/nested"]);
    }

    /// An entry the index refuses (an escaping path) is absent from the
    /// reconstruction too. That is the correct direction: such a path
    /// would make `OrganizationPlan::validate_paths` reject the whole
    /// plan, and it is already hidden from every other read of this
    /// session.
    #[test]
    fn organization_entries_omit_paths_the_index_rejected() {
        let session = session_with(&[
            file("keep.bin", 1, 1),
            file("../escape.bin", 1, 1),
            file("/absolute.bin", 1, 1),
        ]);

        let (_, entries) = session.organization_entries();
        let files: Vec<&str> = entries
            .iter()
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(files, vec!["keep.bin"]);
    }

    /// Pins the documented lossiness: the index stores a parsed
    /// timestamp, so a reconstruction cannot hand back the backend's
    /// original date string. Nothing in the organization engine reads
    /// the field -- this exists so a future reader who wants to is
    /// warned by a failing test rather than by wrong output.
    #[test]
    fn organization_entries_do_not_reconstruct_a_modified_timestamp() {
        let session = session_with(&[file_with_modified("dated.bin", "2024-01-15 10:00:00")]);

        let (_, entries) = session.organization_entries();
        let dated = entries
            .iter()
            .find(|entry| entry.path == "dated.bin")
            .expect("the file must be present");
        assert!(dated.modified.is_none());
    }

    #[test]
    fn organization_entries_track_a_reindex() {
        let session = session_with(&[file("before.bin", 1, 1)]);
        let revision = session.reindex(&[file("after.bin", 2, 2)]);

        let (reported_revision, entries) = session.organization_entries();
        assert_eq!(reported_revision, revision);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| !entry.is_dir)
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["after.bin"]
        );
    }

    #[test]
    fn metadata_reports_what_a_plugin_last_wrote() {
        let session = session_with(&[file("a.bin", 1, 1)]);
        assert!(session.metadata().is_none());

        session.set_metadata(Some(serde_json::json!({ "product_id": "X1" })));
        assert_eq!(
            session
                .metadata()
                .and_then(|value| value.get("product_id").cloned()),
            Some(serde_json::json!("X1"))
        );
    }
}
