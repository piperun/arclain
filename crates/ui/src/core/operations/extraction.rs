//! Extraction as an application-facade operation.
//!
//! The facade owns process spawning and cancellation for the CLI
//! extraction this drives (`arclain_app::operations::extract`) --
//! egui no longer holds a `std::process::Child` directly. Progress,
//! challenges (a wrong-password retry), and completion all route back
//! onto `tab.extraction_dialog()` through
//! `crate::core::operation_bridge`.

use crate::core::tabs::TabState;
use crate::shared::SharedState;
use std::sync::Arc;

/// Resolves user-selected archive-relative paths to the facade's
/// `EntryId`s against an already-fetched directory listing.
///
/// Returns `Err` when `entry_paths` is non-empty but none of them
/// matched an entry in `page`. `ExtractRequest` treats an empty
/// `entry_ids` list as "extract the whole archive" (see
/// `arclain_app::operations::extract`'s `resolve_selection`), so
/// silently returning an empty `Vec` here -- as a `filter_map` over an
/// all-unmatched selection naturally does -- would turn "the user's
/// selection matched nothing in this listing" into "extract
/// everything", inverting the user's intent. Failing loudly here means
/// that can never happen: no operation is ever started from an
/// unresolvable selection.
///
/// Paths that partially match still extract the ones that did -- this
/// only guards the all-unmatched case, which is the one that would
/// otherwise widen to the entire archive.
fn resolve_entry_ids(
    entry_paths: &[String],
    page: &arclain_app::archive::EntryPage,
) -> Result<Vec<arclain_app::ids::EntryId>, String> {
    if entry_paths.is_empty() {
        return Ok(Vec::new());
    }
    let resolved: Vec<_> = entry_paths
        .iter()
        .filter_map(|selected| {
            page.entries
                .iter()
                .find(|entry| entry.path.as_str() == selected)
                .map(|entry| entry.id)
        })
        .collect();
    if resolved.is_empty() {
        return Err(format!(
            "none of the {} selected item(s) could be found in the current archive listing",
            entry_paths.len()
        ));
    }
    Ok(resolved)
}

/// Logs, surfaces a status-bar message, and hides `tab`'s extraction
/// dialog. Shared by every path in `start_extraction` that bails out
/// before an operation is ever started -- without the dialog reset,
/// the dialog would stay stuck showing "Running" forever, since no
/// operation was registered with the bridge to ever drive it to a
/// terminal state.
fn fail_extraction(shared: &SharedState, tab: &TabState, message: String) {
    tracing::error!("[extraction] {message}");
    shared.signals().status_bar.update(|s| {
        s.message = format!("Extraction failed: {message}");
    });
    let mut dialog = tab.extraction_dialog().get();
    dialog.show = false;
    tab.extraction_dialog().set(dialog);
}

/// Starts extracting `entry_paths` (empty means the whole archive) from
/// `tab`'s open archive into a user-picked destination folder.
/// Fire-and-forget: resolves the archive-relative paths to the
/// facade's `EntryId`s (via `list_entries` on the tab's current
/// directory -- selection is always scoped to entries visible in that
/// directory, matching the pre-facade UI's own selection model), then
/// dispatches `start_extract` and registers the resulting operation with
/// the bridge.
pub fn start_extraction(shared: &SharedState, tab: &Arc<TabState>, entry_paths: Vec<String>) {
    // Guards on the operation actually being tracked, not on the
    // dialog's visibility -- the dialog is just a view over that state;
    // checking it directly would incorrectly allow a second, concurrent
    // extraction to start if the dialog were ever hidden (or not yet
    // shown) while an operation was still genuinely in flight.
    if tab.active_extraction_operation.get().is_some() {
        shared.signals().status_bar.update(|s| {
            s.message = "Another extraction is already running".to_string();
        });
        return;
    }
    let Some(session_id) = tab.archive_session_id.get() else {
        shared.signals().status_bar.update(|s| {
            s.message = "No archive open".to_string();
        });
        return;
    };
    let Some(destination) = rfd::FileDialog::new().pick_folder() else {
        return;
    };
    let Some(app) = shared.facade.clone() else {
        tracing::error!("[extraction] start_extraction: no application facade available");
        return;
    };

    let runtime = shared.services.tokio_runtime.clone();
    let shared = shared.clone();
    let tab_id = tab.id;
    let tab = tab.clone();
    let current_directory = tab.listing.get().directory().clone();

    {
        // Built from `default()` rather than mutating whatever the
        // previous extraction left behind: `processed_text`/`error`/
        // `file_action`/`dest_path`/etc. are not otherwise touched here,
        // and a prior extraction's stale values (a leftover error
        // message, an old file count) must not bleed into a fresh run's
        // display before its own first progress event arrives.
        let dialog = crate::shared::dialogs::ExtractionProgressDialog {
            show: true,
            title: format!("Extracting to {}", destination.display()),
            status: crate::shared::dialogs::ExtractionStatus::Running,
            // No facade-level pause/minimize primitive exists (only
            // cancellation) -- disable both rather than leave them
            // silently non-functional. See `crate::core::arclain_app::
            // dialog_handler`'s own comment on the now-inert
            // `Minimized`/`Paused`/`Resumed` dialog results.
            can_pause: false,
            can_minimize: false,
            can_cancel: true,
            started_at: Some(std::time::Instant::now()),
            ..Default::default()
        };
        tab.extraction_dialog().set(dialog);
    }

    runtime.spawn(async move {
        let entry_ids = if entry_paths.is_empty() {
            Vec::new()
        } else {
            let page = match app
                .list_entries(
                    session_id,
                    arclain_app::archive::ListEntriesRequest {
                        directory: current_directory,
                        sort_key: arclain_app::archive::EntrySortKey::Name,
                        sort_direction: arclain_app::archive::SortDirection::Ascending,
                        name_filter: None,
                        offset: 0,
                        limit: crate::core::tabs::ALL_ENTRIES_IN_ONE_DIRECTORY,
                    },
                )
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    fail_extraction(&shared, &tab, format!("{error:?}"));
                    return;
                }
            };
            match resolve_entry_ids(&entry_paths, &page) {
                Ok(ids) => ids,
                Err(message) => {
                    fail_extraction(&shared, &tab, message);
                    return;
                }
            }
        };

        match app
            .start_extract(arclain_app::operations::ExtractRequest {
                session_id,
                entry_ids,
                destination,
                collision_policy: arclain_app::operations::CollisionPolicy::Overwrite,
            })
            .await
        {
            Ok(operation_id) => {
                tab.active_extraction_operation.set(Some(operation_id));
                crate::core::operation_bridge::register_operation(&shared, operation_id, tab_id)
                    .await;
            }
            Err(error) => {
                fail_extraction(&shared, &tab, format!("{error:?}"));
            }
        }
    });
}

/// Extract a set of files identified by archive-root-relative paths from
/// the active tab's archive.
///
/// Callers pre-compute the list of paths — either by filtering the
/// active tab's `browser_entries` against `browser_view_state.selection`
/// (toolbar "Extract" button), or by passing a single path from a
/// per-row action (`file_ops_service::extract`).
pub fn extract_selected(shared: &SharedState, tab: &Arc<TabState>, selected_files: Vec<String>) {
    if selected_files.is_empty() {
        shared.signals().status_bar.update(|s| {
            s.message = "No files selected".to_string();
        });
        return;
    }
    start_extraction(shared, tab, selected_files);
}

/// Extract all files from the archive.
pub fn extract_all(shared: &SharedState, tab: &Arc<TabState>) {
    start_extraction(shared, tab, Vec::new());
}

/// Cancels the extraction currently running for `tab`, if any.
pub fn cancel_extraction(shared: &SharedState, tab: &Arc<TabState>) {
    let Some(operation_id) = tab.active_extraction_operation.get() else {
        return;
    };
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let runtime = shared.services.tokio_runtime.clone();
    runtime.spawn(async move {
        let _ = app.cancel_operation(operation_id).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_app::archive::{ArchiveEntryDto, ArchivePath, EntryKind, EntryPage};
    use arclain_app::ids::{ArchiveSessionId, EntryId};

    fn entry(id: u64, path: &str) -> ArchiveEntryDto {
        ArchiveEntryDto {
            id: EntryId::from_raw(id),
            path: ArchivePath::parse(path).expect("valid test path"),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            kind: EntryKind::File,
            compressed_size: Some(10),
            uncompressed_size: 10,
            modified_at_unix_ms: None,
            encrypted: false,
            crc32: None,
        }
    }

    fn page(entries: Vec<ArchiveEntryDto>) -> EntryPage {
        EntryPage {
            session_id: ArchiveSessionId::from_raw(1),
            revision: 1,
            directory: ArchivePath::root(),
            total: entries.len() as u64,
            entries,
        }
    }

    #[test]
    fn an_empty_selection_resolves_to_the_whole_archive_marker() {
        let listing = page(vec![entry(1, "a.txt")]);
        let resolved = resolve_entry_ids(&[], &listing).expect("empty selection never fails");
        assert!(
            resolved.is_empty(),
            "an empty entry_paths must resolve to an empty entry_ids (whole archive), got {resolved:?}"
        );
    }

    #[test]
    fn every_selected_path_present_in_the_listing_resolves_to_its_entry_id() {
        let listing = page(vec![
            entry(1, "a.txt"),
            entry(2, "b.txt"),
            entry(3, "c.txt"),
        ]);
        let resolved = resolve_entry_ids(&["a.txt".to_string(), "c.txt".to_string()], &listing)
            .expect("both paths are present in the listing");
        assert_eq!(resolved, vec![EntryId::from_raw(1), EntryId::from_raw(3)]);
    }

    #[test]
    fn a_partial_match_still_resolves_the_entries_that_were_found() {
        let listing = page(vec![entry(1, "a.txt")]);
        let resolved = resolve_entry_ids(&["a.txt".to_string(), "gone.txt".to_string()], &listing)
            .expect("at least one path matched, so this must not fail loudly");
        assert_eq!(resolved, vec![EntryId::from_raw(1)]);
    }

    #[test]
    fn a_selection_that_matches_nothing_in_the_listing_fails_loudly_instead_of_widening_to_whole_archive(
    ) {
        let listing = page(vec![entry(1, "a.txt")]);
        // Every selected path is stale/unmatched -- e.g. the browser's
        // cached entries and the freshly-fetched listing disagree.
        // `filter_map` alone would silently collapse this to an empty
        // Vec, which `ExtractRequest` reads as "whole archive". This
        // must be a loud `Err`, never a silent `Ok(vec![])`.
        let result = resolve_entry_ids(
            &["gone.txt".to_string(), "also_gone.txt".to_string()],
            &listing,
        );
        assert!(
            result.is_err(),
            "an all-unmatched non-empty selection must fail loudly, not resolve to \
             an empty entry_ids that `ExtractRequest` would read as \"whole archive\""
        );
    }
}
