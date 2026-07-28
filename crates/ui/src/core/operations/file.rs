use crate::core::tabs::view_state::RevisionedSelection;
use crate::core::tabs::TabId;
use crate::shared::models::file_entry::FileEntry;
use crate::shared::SharedState;
use arclain_app::archive::{ArchivePath, EntrySortKey, ListEntriesRequest, SortDirection};
use arclain_app::ids::ArchiveSessionId;
use arclain_app::operations::ArchiveMutationRequest;
use std::path::PathBuf;

/// How many entries a directory-scoped `list_entries` call requests when
/// resolving a single archive path to its `EntryId` -- see
/// [`start_replace_text`]. Mirrors `FileOpsService`'s identical constant
/// for delete's own path-to-id resolution.
const ALL_ENTRIES_IN_ONE_DIRECTORY: u32 = u32::MAX;

/// Opens a file picker and adds the chosen files to `tab_id`'s open
/// archive through the application facade -- see [`start_add_files`] for
/// the actual dispatch. A no-op (after showing a status message) if the
/// tab has no archive open; the toolbar's own "Add" button already
/// disables itself in that case (`ctx.archive_loaded`), so this guard
/// only matters for a call site that bypasses the button (there is none
/// today, but the check costs nothing and avoids a confusing silent
/// no-op).
pub fn add_files(shared: &SharedState, tab_id: TabId) {
    let tabs = shared.signals().tabs.get();
    let Some(tab) = tabs.get(tab_id) else {
        return;
    };
    let Some(session_id) = tab.archive_session_id.get() else {
        shared.signals().status_bar.update(|status| {
            status.message = "No archive loaded".to_string();
        });
        return;
    };
    let Some(files) = rfd::FileDialog::new().pick_files() else {
        return;
    };
    start_add_files(shared, tab_id, session_id, files);
}

/// Fire-and-forget: submits `source_paths` as an `AddFiles` mutation
/// against `session_id` through the application facade at its current
/// revision, then registers the resulting operation with the bridge so
/// its `SnapshotChanged`/terminal events route back to `tab_id` --
/// mirrors `crate::core::operations::archive::start_archive_open`'s
/// exact fire-and-forget shape. A no-op if `source_paths` is empty (no
/// point round-tripping to the facade for nothing to add) or if no
/// application facade is available (test fixtures that skip a full
/// `ArclainApp::bootstrap`).
///
/// Shared by the toolbar "Add" button (via [`add_files`] above) and
/// routing a native file drop onto an already-open archive (see
/// `crate::features::archive_operations::application::drag_drop::
/// should_add_to_open_archive`, which decides *when* a dropped file
/// should come here instead of opening as a new archive).
pub fn start_add_files(
    shared: &SharedState,
    tab_id: TabId,
    session_id: ArchiveSessionId,
    source_paths: Vec<PathBuf>,
) {
    if source_paths.is_empty() {
        return;
    }
    let Some(app) = shared.facade.clone() else {
        tracing::error!("[file] start_add_files: no application facade available");
        return;
    };
    let shared = shared.clone();
    let runtime = shared.services.tokio_runtime.clone();
    runtime.spawn(async move {
        let expected_revision = match app.archive_snapshot(session_id).await {
            Ok(snapshot) => snapshot.revision,
            Err(error) => {
                tracing::error!("[file] start_add_files: archive_snapshot failed: {error:?}");
                shared.signals().status_bar.update(|status| {
                    status.message = format!("Add files failed: {}", error.summary);
                });
                return;
            }
        };
        let request = ArchiveMutationRequest::AddFiles {
            session_id,
            expected_revision,
            destination: ArchivePath::root(),
            source_paths,
        };
        match app.start_archive_mutation(request).await {
            Ok(operation_id) => {
                crate::core::operation_bridge::register_operation(&shared, operation_id, tab_id)
                    .await;
            }
            Err(error) => {
                tracing::error!("[file] start_archive_mutation (AddFiles) was rejected: {error:?}");
                shared.signals().status_bar.update(|status| {
                    status.message = format!("Add files failed: {}", error.summary);
                });
            }
        }
    });
}

/// Fire-and-forget: submits `content` as a `ReplaceText` mutation for
/// the entry currently named `path_in_archive`, against `session_id`'s
/// open archive through the application facade at its current revision,
/// then registers the resulting operation with the bridge -- the file-
/// edit dialog's save action. Mirrors [`start_add_files`]'s exact shape.
///
/// Always saves back to `path_in_archive` itself: the facade's
/// `ReplaceText` mutation has no rename/move concept (the contract's
/// `ArchiveMutationRequest` only ever replaces an existing entry's own
/// content). The file-edit dialog's own editable name field can differ
/// from `path_in_archive` if the user typed a different one -- detecting
/// that and telling the user their rename was not honored is the
/// caller's job (`crate::core::arclain_app::dialog_handler`'s
/// `FileEditResult::Save` handler), done synchronously and durably
/// there (via the dialog's own `error` field) rather than through a
/// transient status-bar write from this fire-and-forget task, which a
/// completion event racing it on a background task could silently
/// clobber before the user ever saw it.
pub fn start_replace_text(
    shared: &SharedState,
    tab_id: TabId,
    session_id: ArchiveSessionId,
    path_in_archive: String,
    content: String,
) {
    let Some(app) = shared.facade.clone() else {
        tracing::error!("[file] start_replace_text: no application facade available");
        return;
    };
    let shared = shared.clone();
    let runtime = shared.services.tokio_runtime.clone();
    runtime.spawn(async move {
        let parent = path_in_archive
            .rsplit_once('/')
            .map(|(parent, _)| parent.to_string())
            .unwrap_or_default();
        let directory = ArchivePath::parse(parent).unwrap_or_else(|_| ArchivePath::root());
        let page = match app
            .list_entries(
                session_id,
                ListEntriesRequest {
                    directory,
                    sort_key: EntrySortKey::Name,
                    sort_direction: SortDirection::Ascending,
                    name_filter: None,
                    offset: 0,
                    limit: ALL_ENTRIES_IN_ONE_DIRECTORY,
                },
            )
            .await
        {
            Ok(page) => page,
            Err(error) => {
                tracing::error!("[file] start_replace_text: list_entries failed: {error:?}");
                shared.signals().status_bar.update(|status| {
                    status.message = format!("Save failed: {}", error.summary);
                });
                return;
            }
        };
        let Some(entry) = page
            .entries
            .iter()
            .find(|entry| entry.path.as_str() == path_in_archive)
        else {
            shared.signals().status_bar.update(|status| {
                status.message =
                    "Save failed: the file no longer exists in the archive".to_string();
            });
            return;
        };
        let request = ArchiveMutationRequest::ReplaceText {
            session_id,
            expected_revision: page.revision,
            entry_id: entry.id,
            content,
        };
        match app.start_archive_mutation(request).await {
            Ok(operation_id) => {
                crate::core::operation_bridge::register_operation(&shared, operation_id, tab_id)
                    .await;
            }
            Err(error) => {
                tracing::error!(
                    "[file] start_archive_mutation (ReplaceText) was rejected: {error:?}"
                );
                shared.signals().status_bar.update(|status| {
                    status.message = format!("Save failed: {}", error.summary);
                });
            }
        }
    });
}

/// Derive the exact file paths affected by a toolbar delete action.
///
/// Selection may intentionally outlive a search filter, but destructive
/// toolbar actions apply only to selected rows visible under the current
/// search. Folder rows retain the existing non-deletable behavior.
pub(crate) fn selected_file_paths_for_search(
    entries: &[FileEntry],
    selection: &RevisionedSelection,
    search: &str,
) -> Vec<String> {
    let filter = search.trim().to_lowercase();
    entries
        .iter()
        .filter(|entry| {
            !entry.is_folder
                && selection.contains(&entry.archive_path)
                && (filter.is_empty() || entry.name.to_lowercase().contains(&filter))
        })
        .map(|entry| entry.archive_path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_folder: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: name.to_string(),
            archive_path: name.to_string(),
            size: "0 B".to_string(),
            compressed: "0 B".to_string(),
            ratio: "0%".to_string(),
            modified: String::new(),
            crc32: String::new(),
            encrypted: false,
            is_folder,
        }
    }

    #[test]
    fn filtered_delete_emits_only_visible_selected_file_paths() {
        let mut entries = vec![
            entry("visible.txt", false),
            entry("hidden.txt", false),
            entry("visible-folder", true),
        ];
        entries[0].archive_path = "A/visible.txt".to_string();
        entries[1].archive_path = "A/hidden.txt".to_string();
        entries[2].archive_path = "A/visible-folder".to_string();
        let mut selection = RevisionedSelection::default();
        selection.extend([
            "A/visible.txt".to_string(),
            "A/hidden.txt".to_string(),
            "A/visible-folder".to_string(),
        ]);

        let paths = selected_file_paths_for_search(&entries, &selection, " visible ");

        assert_eq!(paths, vec!["A/visible.txt"]);
        assert!(selection.contains("A/hidden.txt"));
    }

    #[test]
    fn unfiltered_delete_emits_every_selected_file_path() {
        let entries = vec![
            entry("first.txt", false),
            entry("second.txt", false),
            entry("folder", true),
        ];
        let mut selection = RevisionedSelection::default();
        selection.extend([
            "first.txt".to_string(),
            "second.txt".to_string(),
            "folder".to_string(),
        ]);

        let paths = selected_file_paths_for_search(&entries, &selection, "  ");

        assert_eq!(paths, vec!["first.txt", "second.txt"]);
    }
}
