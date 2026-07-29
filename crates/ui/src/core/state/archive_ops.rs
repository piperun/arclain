//! Archive operations - reading files inside an already-open archive.
//!
//! Opening an archive itself (`list_archive`/`list_with_password` in
//! earlier revisions of this file) moved onto
//! `arclain_app::ArclainApp::start_open_archive`, driven by
//! `crate::core::operation_bridge` and started via
//! `crate::core::operations::archive::start_archive_open` -- see that
//! module for the full open flow.
//!
//! The archive *mutations* that used to live here too -- `add_files_to_archive`,
//! `delete_files`, `add_or_update_file_from_str` -- moved onto
//! `arclain_app::ArclainApp::start_archive_mutation`
//! (`arclain_app::operations::ArchiveMutationRequest`), driven by
//! `crate::core::operations::file::start_add_files`,
//! `crate::features::archive_browser::application::FileOpsService::delete_files`,
//! and `crate::core::arclain_app::dialog_handler`'s file-edit save
//! handler respectively -- each now runs through the application's own
//! cancellable, capability-gated, revision-checked operation registry
//! instead of calling a backend directly and synchronously on whichever
//! thread happened to call in. `read_text_file` (the file-edit dialog's
//! *read* side, as opposed to its save) was already dead code by the
//! time this task began -- `FileOpsService::read_text`/`read_text_with_io`
//! is the one live "read for edit" path, and it is unaffected by this
//! move: reading a file's content to populate the editor still goes
//! straight through the session/backend, exactly as it already did.
//!
//! What remains here is the one flat, pre-facade read query the archive
//! browser's still-not-yet-migrated flat `entries`/`browser_entries`
//! model is built around.

use super::AppState;

impl AppState {
    /// TRANSITIONAL(4c): the active tab's current directory, in the
    /// pre-facade flat entry shape.
    ///
    /// Reads the tab's own [`arclain_app::archive::EntryPage`] and
    /// converts each row back down -- the page *is* the current
    /// directory's listing, so no filtering is needed, and the rows carry
    /// the session's own folder aggregates rather than this call
    /// recomputing them. Empty until the tab holds a page for the
    /// directory it is showing.
    ///
    /// Three deliberate differences from the flat filter this replaced:
    /// `path` is the archive-root path (the identity every live consumer
    /// keys on) rather than the path relative to the displayed folder;
    /// `modified` is re-rendered from the parsed timestamp (see
    /// [`crate::core::utils::format_modified_unix_ms`]), which differs
    /// only for a backend whose own date string the facade could not
    /// parse; and a directory's `size`/`packed_size` are the session
    /// index's recursive aggregates, which is what the pre-facade filter
    /// computed for a folder row too.
    pub fn get_current_entries(&self) -> Vec<arclain_core::ArchiveEntry> {
        let tab = self.signals.tabs.get().active().clone();
        tab.listing
            .get()
            .entries()
            .iter()
            .map(core_entry_from_dto)
            .collect()
    }
}

/// TRANSITIONAL(4c): one pre-facade flat entry rebuilt from a facade
/// listing row. Goes with [`AppState::get_current_entries`].
fn core_entry_from_dto(dto: &arclain_app::archive::ArchiveEntryDto) -> arclain_core::ArchiveEntry {
    let modified = crate::core::utils::format_modified_unix_ms(dto.modified_at_unix_ms);
    arclain_core::ArchiveEntry {
        path: dto.path.as_str().to_string(),
        size: dto.uncompressed_size,
        packed_size: dto.compressed_size.unwrap_or(0),
        modified: (!modified.is_empty()).then_some(modified),
        is_dir: matches!(dto.kind, arclain_app::archive::EntryKind::Directory),
        encrypted: dto.encrypted,
        crc32: dto.crc32.clone(),
    }
}
