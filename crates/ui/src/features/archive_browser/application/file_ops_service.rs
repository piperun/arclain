//! File operations application service

use crate::core::operations;
use crate::core::tabs::{OpGuard, TabState};
use crate::features::archive_operations::ArchiveOperationsState;
use crate::features::file_editing::domain::types::FileEditLoadState;
use crate::shared::SharedState;
use anyhow::{anyhow, Result};
use arclain_app::archive::{ArchivePath, EntryKind, ListEntriesRequest};
use arclain_app::ids::ArchiveSessionId;
use arclain_app::operations::ArchiveMutationRequest;
use arclain_app::ArclainApp;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Narrow I/O boundary used to verify that a text read is scheduled
/// rather than executed by the caller. Public only because integration
/// tests compile as a separate crate.
///
/// Delete used to have an equivalent injectable seam here
/// (`delete_and_list`/`DeleteListResult`) before it moved onto
/// `arclain_app::ArclainApp::start_archive_mutation` (see
/// [`FileOpsService::delete_files`]'s own doc comment) -- that seam is
/// gone now: the facade's own integration tests
/// (`crates/app/tests/archive_mutation.rs`) exercise a real,
/// injectable-at-bootstrap fake backend, and this crate's own
/// `crates/ui/tests/` cover the UI-side wiring end to end against a real
/// bootstrapped `ArclainApp`.
///
/// What survives here is a *scheduling* seam, not a backend one: the
/// read itself is `ArclainApp::read_entry_text`, and what this boundary
/// still buys is deterministic control over *when* a read finishes, so
/// the two properties that have nothing to do with archives -- a caller
/// that is never blocked on I/O, and a stale completion that cannot
/// overwrite a newer read -- stay pinned without racing a real archive.
/// It returns a future because the facade call is `async`; a
/// production implementation performs no blocking work of its own.
#[doc(hidden)]
pub type TextReadFuture<'a> = Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;

#[doc(hidden)]
pub trait TextReadIo: Send + Sync {
    /// Reads the archive-root-relative `path`'s text out of the tab's
    /// own archive session.
    fn read_text(&self, path: String) -> TextReadFuture<'_>;
}

/// Reads through the application facade: resolves `path` to the
/// `EntryId` its own session minted, then asks the session -- which
/// supplies the backend and the password it was opened with -- for the
/// entry's text.
///
/// The id is never fabricated here. It comes out of a `list_entries`
/// page of the file's own containing directory, exactly as every other
/// path-to-id resolution in this crate does, because a frontend-minted
/// id could name a different entry than the row the user clicked.
struct FacadeTextReadIo {
    app: ArclainApp,
    session_id: ArchiveSessionId,
}

impl TextReadIo for FacadeTextReadIo {
    fn read_text(&self, path: String) -> TextReadFuture<'_> {
        Box::pin(async move {
            let (directory, name) = split_archive_path(&path)?;
            let page = self
                .app
                .list_entries(
                    self.session_id,
                    ListEntriesRequest::whole_directory(directory),
                )
                .await
                .map_err(|error| anyhow!("{}", error.summary))?;
            let entry = readable_entry_named(&page.entries, &name)
                .ok_or_else(|| anyhow!("{path} is no longer in the archive"))?;
            self.app
                .read_entry_text(self.session_id, entry.id)
                .await
                .map_err(|error| anyhow!("{}", error.summary))
        })
    }
}

/// The entry named `name` in one directory's listing that could actually
/// hold text.
///
/// A file and a directory can share a name at the same nesting level --
/// that collision is precisely what the session's `(kind, path, ordinal)`
/// entry identity exists to disambiguate -- so a page can legitimately
/// contain two rows answering to `name`, and only one of them is
/// readable.
///
/// Today's index happens to list the file first (its build inserts every
/// file before any directory, and the name sort that follows is stable),
/// so a name-only match would pick the right row by accident. Nothing
/// promises that: it is a tie-break artifact of build order, not a
/// documented property of `list_entries`. Matching on kind makes the
/// selection independent of it, and mirrors the kind check
/// `file_opener`'s `resolve_directory_entry_id` already applies from the
/// other side. Without it, the day the order changes, reading a file
/// fails with `InvalidInput` ("a directory entry has no text content to
/// read") about a row sitting right there in the same page.
fn readable_entry_named<'a>(
    entries: &'a [arclain_app::archive::ArchiveEntryDto],
    name: &str,
) -> Option<&'a arclain_app::archive::ArchiveEntryDto> {
    entries
        .iter()
        .find(|entry| entry.name == name && entry.kind != EntryKind::Directory)
}

/// Stands in when the tab holds no archive session (or the composition
/// has no facade at all). The dialog still opens and reports the refusal
/// through the same path a backend failure takes, rather than the read
/// silently doing nothing.
struct NoSessionTextReadIo;

impl TextReadIo for NoSessionTextReadIo {
    fn read_text(&self, _path: String) -> TextReadFuture<'_> {
        Box::pin(std::future::ready(Err(anyhow!("No archive loaded"))))
    }
}

/// Splits an archive-root-relative path into the directory to list and
/// the entry name to find inside it. Both halves come straight from the
/// row the user clicked, so a path the archive path type refuses is a
/// caller error rather than something to normalize away silently.
fn split_archive_path(path: &str) -> Result<(ArchivePath, String)> {
    let normalized = path.replace('\\', "/");
    let (directory, name) = match normalized.rfind('/') {
        Some(position) => (&normalized[..position], &normalized[position + 1..]),
        None => ("", normalized.as_str()),
    };
    let directory =
        ArchivePath::parse(directory.to_string()).map_err(|error| anyhow!("{}", error.summary))?;
    Ok((directory, name.to_string()))
}

pub struct FileOpsService;

impl FileOpsService {
    /// Fire-and-forget: resolves `paths` (archive-relative path strings,
    /// scoped to `origin`'s currently-viewed directory -- the only rows a
    /// selection can ever contain) to their `EntryId`s, then submits a
    /// `DeleteEntries` mutation through the application facade at the
    /// resolved listing's own revision. The bridge
    /// (`crate::core::operation_bridge`) refreshes `origin`'s
    /// entries/browser_entries once the operation reaches `Completed`
    /// -- this method itself never touches those signals.
    ///
    /// Delete used to call `ArchiveBackend::delete_files` directly here,
    /// synchronously inside `spawn_blocking`, serialized per tab via
    /// `origin.archive_edit_lock` (a `parking_lot::Mutex` that also
    /// serialized this against a concurrent save-edit on the *same*
    /// tab, but not against a delete/edit on a *different* tab that
    /// happened to share the same archive file). `ArchiveSession::
    /// mutation_lock` (an async, session-scoped lock the facade now
    /// owns) supersedes that role: it serializes every mutation kind
    /// against every other one on the same session, regardless of which
    /// tab initiated it.
    pub fn delete_files(&self, shared: &SharedState, origin: Arc<TabState>, paths: Vec<String>) {
        if paths.is_empty() {
            shared.signals().status_bar.update(|status| {
                status.message = "No files selected".to_string();
            });
            return;
        }
        let Some(session_id) = origin.archive_session_id.get() else {
            shared.signals().status_bar.update(|status| {
                status.message = "No archive loaded".to_string();
            });
            return;
        };
        let Some(app) = shared.facade.clone() else {
            tracing::error!("[file_ops] delete_files: no application facade available");
            return;
        };

        let tab_id = origin.id;
        let directory = origin.listing.get().directory().clone();
        let shared = shared.clone();
        let runtime = shared.services.tokio_runtime.clone();
        runtime.spawn(async move {
            let page = match app
                .list_entries(
                    session_id,
                    arclain_app::archive::ListEntriesRequest::whole_directory(directory),
                )
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    tracing::error!("[file_ops] delete_files: list_entries failed: {error:?}");
                    shared.signals().status_bar.update(|status| {
                        status.message = format!("Delete failed: {}", error.summary);
                    });
                    return;
                }
            };

            let wanted: std::collections::HashSet<&str> =
                paths.iter().map(String::as_str).collect();
            let entry_ids: Vec<arclain_app::ids::EntryId> = page
                .entries
                .iter()
                .filter(|entry| wanted.contains(entry.path.as_str()))
                .map(|entry| entry.id)
                .collect();
            if entry_ids.is_empty() {
                // Every selected path has already disappeared from the
                // current listing (deleted by another action in the
                // meantime, or the selection was stale) -- nothing left
                // to delete.
                shared.signals().status_bar.update(|status| {
                    status.message = "No matching entries found to delete".to_string();
                });
                return;
            }

            let request = ArchiveMutationRequest::DeleteEntries {
                session_id,
                expected_revision: page.revision,
                entry_ids,
            };
            match app.start_archive_mutation(request).await {
                Ok(operation_id) => {
                    crate::core::operation_bridge::register_operation(
                        &shared,
                        operation_id,
                        tab_id,
                    )
                    .await;
                }
                Err(error) => {
                    tracing::error!(
                        "[file_ops] start_archive_mutation (DeleteEntries) was rejected: {error:?}"
                    );
                    shared.signals().status_bar.update(|status| {
                        status.message = format!("Delete failed: {}", error.summary);
                    });
                }
            }
        });
    }

    pub fn extract(
        &self,
        shared: &SharedState,
        _ops_state: &mut ArchiveOperationsState,
        file: &str,
    ) {
        // Per-row extract receives the stable archive-root path and
        // bypasses selection entirely — extract_selected takes a list
        // of paths so we just hand it `[file]`.
        let active_tab = shared.signals().tabs.get().active().clone();
        operations::extraction::extract_selected(shared, &active_tab, vec![file.to_string()]);
    }

    /// Loads `path`'s text into `origin`'s file-edit dialog, through the
    /// archive session `origin` holds.
    ///
    /// Refuses before touching the dialog when the tab has no open
    /// session: without one there is no archive to read from, and the
    /// pre-facade path's own equivalent -- no `archive_path` -- reported
    /// the same way.
    pub fn read_text(&self, shared: &SharedState, origin: Arc<TabState>, path: String) {
        let io: Arc<dyn TextReadIo> = match (origin.archive_session_id.get(), shared.facade.clone())
        {
            (Some(session_id), Some(app)) => Arc::new(FacadeTextReadIo { app, session_id }),
            _ => Arc::new(NoSessionTextReadIo),
        };
        self.read_text_with_io(shared, origin, path, io);
    }

    #[doc(hidden)]
    pub fn read_text_with_io(
        &self,
        shared: &SharedState,
        origin: Arc<TabState>,
        path: String,
        io: Arc<dyn TextReadIo>,
    ) {
        let request_id = origin.file_request_seq.fetch_add(1, Ordering::Relaxed) + 1;
        origin.file_edit_dialog.update(|dialog| {
            dialog.show = true;
            dialog.full_path_in_archive = path.clone();
            dialog.name_input = path.clone();
            dialog.content.clear();
            dialog.original_content.clear();
            dialog.error.clear();
            dialog.load_state = FileEditLoadState::Loading { request_id };
        });

        let Some(archive) = origin.archive_path.get() else {
            let message = "No archive loaded".to_string();
            origin.file_edit_dialog.update(|dialog| {
                if dialog.full_path_in_archive == path
                    && dialog.load_state == (FileEditLoadState::Loading { request_id })
                {
                    dialog.error = message.clone();
                    dialog.load_state = FileEditLoadState::Failed(message.clone());
                }
            });
            shared.signals().status_bar.update(|status| {
                status.message = message;
            });
            return;
        };

        let runtime = shared.services.tokio_runtime.clone();
        let status_bar = shared.signals().status_bar.clone();
        let guard = OpGuard::new(&origin);

        runtime.spawn(async move {
            let _guard = guard;
            // No `spawn_blocking` hop here: the read is a facade call
            // that owns its own blocking discipline (see
            // `ArclainApp::read_entry_text`), so wrapping it would park
            // a blocking-pool thread on a future that never blocks.
            let result = io.read_text(path.clone()).await;

            if origin.archive_path.get().as_ref() != Some(&archive) {
                return;
            }

            let mut applied_error = None;
            origin.file_edit_dialog.update(|dialog| {
                if dialog.full_path_in_archive != path
                    || dialog.load_state != (FileEditLoadState::Loading { request_id })
                {
                    return;
                }

                match result {
                    Ok(content) => {
                        dialog.content = content.clone();
                        dialog.original_content = content;
                        dialog.error.clear();
                        dialog.load_state = FileEditLoadState::Ready;
                    }
                    Err(error) => {
                        let message = format!("Failed to read file: {error}");
                        dialog.error = message.clone();
                        dialog.load_state = FileEditLoadState::Failed(message.clone());
                        applied_error = Some(message);
                    }
                }
            });

            if let Some(message) = applied_error {
                crate::core::utils::log_failure("FileEdit", &message);
                status_bar.update(|status| status.message = message);
            }
        });
    }

    pub fn copy_path(&self, egui_ctx: &egui::Context, file: &str) {
        egui_ctx.copy_text(file.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_app::archive::ArchiveEntryDto;

    /// Test-only rows standing in for one `list_entries` page. The
    /// `EntryId`s are fabricated but never leave this module -- the
    /// function under test only *selects* a row, and nothing here hands
    /// an id to a facade.
    fn row(id: u64, path: &str, kind: EntryKind) -> ArchiveEntryDto {
        ArchiveEntryDto {
            id: arclain_app::ids::EntryId::from_raw(id),
            path: ArchivePath::parse(path.to_string()).unwrap(),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            kind,
            compressed_size: Some(0),
            uncompressed_size: 0,
            modified_at_unix_ms: None,
            encrypted: false,
            crc32: None,
        }
    }

    /// The collision the session's entry identity exists to disambiguate:
    /// `notes` is both a file and (implied by `notes/inner.txt`) a
    /// directory. The directory is deliberately listed *first* here --
    /// the order a name-only match would lose to. A real page currently
    /// happens to order them the other way round, which is exactly why
    /// this case needs a test that does not depend on that: it pins the
    /// selection rule itself, not the ordering that presently makes the
    /// rule redundant.
    #[test]
    fn a_directory_sharing_the_targets_name_never_wins_the_read() {
        let page = [
            row(1, "notes", EntryKind::Directory),
            row(2, "notes", EntryKind::File),
        ];

        let chosen = readable_entry_named(&page, "notes").expect("the file row must be found");
        assert_eq!(chosen.kind, EntryKind::File);
        assert_eq!(chosen.id, arclain_app::ids::EntryId::from_raw(2));
    }

    /// A symlink is readable text as far as the backend is concerned --
    /// only a directory is excluded.
    #[test]
    fn a_symlink_is_still_a_readable_target() {
        let page = [row(1, "link", EntryKind::Symlink)];

        assert_eq!(
            readable_entry_named(&page, "link").map(|entry| entry.id),
            Some(arclain_app::ids::EntryId::from_raw(1))
        );
    }

    /// A name that only a directory carries has nothing to read, and must
    /// report "not in the archive" rather than resolving to the folder.
    #[test]
    fn a_name_only_a_directory_carries_resolves_to_nothing() {
        let page = [row(1, "notes", EntryKind::Directory)];

        assert!(readable_entry_named(&page, "notes").is_none());
    }

    #[test]
    fn splitting_a_path_separates_the_directory_from_the_entry_name() {
        let (directory, name) = split_archive_path("game/data/save.dat").unwrap();
        assert_eq!(directory.as_str(), "game/data");
        assert_eq!(name, "save.dat");

        let (root, name) = split_archive_path("readme.txt").unwrap();
        assert_eq!(root.as_str(), "");
        assert_eq!(name, "readme.txt");

        // A backslash-separated path normalizes the same way, so a row
        // whose path arrived from a Windows-shaped source still resolves.
        let (directory, name) = split_archive_path(r"game\data\save.dat").unwrap();
        assert_eq!(directory.as_str(), "game/data");
        assert_eq!(name, "save.dat");
    }
}
