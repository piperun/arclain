//! Opens a file from the active tab's archive in the OS's default external
//! application (or, if the extracted content is itself an archive, as a
//! nested archive in this tab instead) -- fire-and-forget, dispatched onto
//! the application facade's `start_materialization` operation and driven
//! to completion by `crate::core::operation_bridge::handle_materialize_completed`.
//!
//! Replaces the pre-facade implementation's leaked `arclain_core::FileOpener`
//! temp directory: that implementation called `std::mem::forget` on its
//! `FileOpener` so the directory it owned would survive past the point the
//! background extraction thread (and, later, the externally-launched
//! viewer) needed it -- see this module's own regression test for a direct
//! reproduction of why the leak was "necessary" without it. That directory
//! was never reclaimed again for the life of the process. This
//! implementation instead materializes through an explicit
//! `arclain_app::materialization::MaterializationLease`: released
//! immediately once its content turns out to be a nested archive (nothing
//! reads it again after that one `list()` call), or else renewed
//! periodically for as long as this session runs
//! (`crate::core::operation_bridge::ExternalOpenLeases`) since there is no
//! reliable way to know when an arbitrary OS-launched external application
//! is done reading it -- reclaimed either way by `ArclainApp::shutdown`'s
//! own cleanup, or by the lease's own expiry if renewal ever stops.

use crate::core::operation_bridge::MaterializationAction;
use crate::shared::SharedState;
use arclain_app::archive::{
    ArchivePath, EntryKind, EntrySortKey, ListEntriesRequest, SortDirection,
};
use arclain_app::ids::{ArchiveSessionId, EntryId};
use arclain_app::materialization::{MaterializationPurpose, MaterializeRequest};
use arclain_app::ArclainApp;

/// Whether opening `file_path` should materialize only that one file, or
/// its whole containing directory too (bringing sibling files along --
/// a game executable's co-located DLLs, a save file's neighboring config).
/// Ported from the pre-facade implementation's exact extension list.
///
/// Not reproduced: the pre-facade `WithDependencies` strategy additionally
/// scanned the *whole* archive for `.dll`/`.config` files outside the
/// target's own directory. That cross-directory heuristic has no
/// equivalent under the facade's per-entry materialization lease (a lease
/// always resolves to one entry's own extracted subtree -- its containing
/// directory, at most -- never an arbitrary scattered file set from
/// elsewhere in the archive); folding it in would mean either a second,
/// separate materialization call per stray dependency file (defeating the
/// point of co-locating them for the launched process to find) or growing
/// the facade's request shape well beyond what this task's contract
/// defines. Materializing the target's own directory still covers the
/// overwhelmingly common real layout (an executable and its DLLs sitting
/// side by side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenStrategy {
    FileOnly,
    SameDirectory,
}

fn determine_extraction_strategy(file_path: &str) -> OpenStrategy {
    let lower = file_path.to_lowercase();
    let self_contained_extensions = [
        // Images
        ".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp", ".svg", ".ico", ".tiff", ".tif",
        // Documents
        ".pdf", ".txt", ".md", ".html", ".htm", ".xml", ".json", ".csv", // Audio
        ".mp3", ".wav", ".flac", ".ogg", ".m4a", ".aac", // Video
        ".mp4", ".mkv", ".avi", ".mov", ".webm", ".wmv",
    ];
    if self_contained_extensions
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        return OpenStrategy::FileOnly;
    }
    // Executables and everything else default to bringing the containing
    // directory along -- matches the pre-facade `WithDependencies`
    // (executables) and `SameDirectory` (the prior default) union, see
    // this type's own doc comment for the one deliberate difference.
    OpenStrategy::SameDirectory
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

fn parent_directory(path: &str) -> String {
    let normalized = normalize(path);
    match normalized.rfind('/') {
        Some(pos) => normalized[..pos].to_string(),
        None => String::new(),
    }
}

fn basename(path: &str) -> String {
    let normalized = normalize(path);
    match normalized.rfind('/') {
        Some(pos) => normalized[pos + 1..].to_string(),
        None => normalized,
    }
}

async fn list_directory(
    app: &ArclainApp,
    session_id: ArchiveSessionId,
    directory: ArchivePath,
) -> Result<arclain_app::archive::EntryPage, arclain_app::error::ApplicationError> {
    app.list_entries(
        session_id,
        ListEntriesRequest {
            directory,
            sort_key: EntrySortKey::Name,
            sort_direction: SortDirection::Ascending,
            name_filter: None,
            offset: 0,
            limit: 100_000,
        },
    )
    .await
}

/// Resolves the `EntryId` of the directory entry named `directory_name`
/// within `parent`'s own listing. `None` if not found, or if nothing at
/// that name is actually a directory -- a file and a directory can share a
/// name at the same nesting level (see `arclain_app::archive`'s
/// `EntryIdAssigner` doc comment), and only a real directory's subtree is
/// something `start_materialization` can expand.
async fn resolve_directory_entry_id(
    app: &ArclainApp,
    session_id: ArchiveSessionId,
    parent: ArchivePath,
    directory_name: &str,
) -> Option<EntryId> {
    let page = list_directory(app, session_id, parent).await.ok()?;
    page.entries
        .iter()
        .find(|entry| entry.name == directory_name && entry.kind == EntryKind::Directory)
        .map(|entry| entry.id)
}

/// What to materialize for one `open_file_from_archive` call: the target
/// file's own `EntryId` (opened directly), or its containing directory's
/// `EntryId` alongside the target's own basename within it (the directory
/// is what gets materialized; the basename locates the specific file to
/// actually launch inside the resulting lease).
async fn resolve_materialization_target(
    app: &ArclainApp,
    session_id: ArchiveSessionId,
    file_path: &str,
    strategy: OpenStrategy,
) -> Result<(EntryId, Option<String>), String> {
    let directory =
        ArchivePath::parse(parent_directory(file_path)).unwrap_or_else(|_| ArchivePath::root());
    let name = basename(file_path);

    let page = list_directory(app, session_id, directory.clone())
        .await
        .map_err(|error| format!("{error:?}"))?;
    let Some(target_entry) = page.entries.iter().find(|entry| entry.name == name) else {
        return Err(format!("{file_path} not found in the archive"));
    };

    if strategy == OpenStrategy::FileOnly || directory.as_str().is_empty() {
        // Either the file's own extension says "just this one file", or
        // there is no containing directory of its own to materialize (the
        // target already sits at the archive root) -- fall back to the
        // file itself either way.
        return Ok((target_entry.id, None));
    }

    let grandparent = ArchivePath::parse(parent_directory(directory.as_str()))
        .unwrap_or_else(|_| ArchivePath::root());
    let directory_name = basename(directory.as_str());
    match resolve_directory_entry_id(app, session_id, grandparent, &directory_name).await {
        Some(directory_entry_id) => Ok((directory_entry_id, Some(name))),
        None => Ok((target_entry.id, None)),
    }
}

/// Opens `file_path` (an archive-root-relative path within the active
/// tab's open archive) in the OS's default external application.
/// Fire-and-forget: resolves the target's `EntryId` (see
/// [`resolve_materialization_target`]), dispatches `start_materialization`,
/// and registers a [`MaterializationAction::ExternalOpen`] the bridge acts
/// on once the operation completes. See the module doc comment for the
/// full lease lifecycle.
pub fn open_file_from_archive(shared: &SharedState, file_path: &str) {
    let tab = shared.signals().tabs.get().active().clone();
    let Some(session_id) = tab.archive_session_id.get() else {
        shared.signals().status_bar.update(|s| {
            s.message = "No archive open".to_string();
        });
        return;
    };
    let Some(app) = shared.facade.clone() else {
        tracing::error!("[file_opener] open_file_from_archive: no application facade available");
        return;
    };

    let strategy = determine_extraction_strategy(file_path);
    let file_path_owned = file_path.to_string();
    let tab_id = tab.id;
    let origins = shared.operation_origins.clone();
    let actions = shared.materialization_actions.clone();
    let shared = shared.clone();

    shared.services.tokio_runtime.clone().spawn(async move {
        let (entry_id, relative_target) = match resolve_materialization_target(
            &app,
            session_id,
            &file_path_owned,
            strategy,
        )
        .await
        {
            Ok(resolved) => resolved,
            Err(message) => {
                tracing::error!("[file_opener] failed to resolve {file_path_owned:?}: {message}");
                shared.signals().status_bar.update(|s| {
                    s.message = format!("Failed to open {file_path_owned}: {message}");
                });
                return;
            }
        };

        match app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_id,
                purpose: MaterializationPurpose::ExternalOpen,
            })
            .await
        {
            Ok(operation_id) => {
                origins.register(operation_id, tab_id);
                actions.register(
                    operation_id,
                    MaterializationAction::ExternalOpen { relative_target },
                );
                shared.signals().status_bar.update(|s| {
                    s.message = format!("Opening {file_path_owned}...");
                });
            }
            Err(error) => {
                tracing::error!("[file_opener] start_materialization was rejected: {error:?}");
                shared.signals().status_bar.update(|s| {
                    s.message = format!("Failed to open {file_path_owned}: {error:?}");
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    /// Regression evidence for the exact bug this module's replacement
    /// fixes, reproduced directly against `arclain_core::FileOpener`
    /// (the struct the pre-facade `open_file_from_archive` implementation
    /// used): dropping a `FileOpener` deletes its temp directory
    /// immediately, even if a background thread (the real extraction
    /// call, or -- in the real, now-replaced code path -- the externally
    /// launched viewer that follows it) still needs files inside it. This
    /// is exactly why the pre-facade code called
    /// `std::mem::forget(opener)`: without it, the sequence below is
    /// precisely what would have happened on every single-file open.
    ///
    /// Deterministic (two `Barrier`s, no sleeps): the background thread
    /// only attempts its write after the main thread has already dropped
    /// `opener`, and the main thread only proceeds past the barrier once
    /// it knows the background thread has started -- there is no timing
    /// window for this test to pass by accident.
    ///
    /// This test is not expected to flip to failing once the replacement
    /// ships: `arclain_core::FileOpener` itself is unchanged (out of this
    /// task's scope; only this module stopped constructing one for real
    /// content). It stays in the suite as permanent, executable
    /// documentation of why the lease-based replacement above is
    /// necessary, not a red-then-green migration marker.
    #[test]
    fn dropping_the_file_opener_immediately_after_spawning_background_extraction_deletes_the_directory_the_thread_still_needs(
    ) {
        use arclain_core::FileOpener;
        use std::sync::Arc;

        let opener = FileOpener::new().expect("FileOpener::new must succeed");
        let temp_dir = opener.temp_dir().to_path_buf();

        let started = Arc::new(std::sync::Barrier::new(2));
        let proceed = Arc::new(std::sync::Barrier::new(2));

        let started_for_thread = started.clone();
        let proceed_for_thread = proceed.clone();
        let temp_dir_for_thread = temp_dir.clone();
        let handle = std::thread::spawn(move || {
            // Mirrors the real background extraction thread having been
            // spawned and about to write into `opener`'s temp directory.
            started_for_thread.wait();
            // Waits until the main thread below has already dropped
            // `opener` -- exactly what happens in the real (unpatched)
            // code, since the function that spawns this thread returns
            // (and drops its local `opener` binding) immediately after
            // spawning, never waiting for the thread to finish.
            proceed_for_thread.wait();
            std::fs::write(temp_dir_for_thread.join("extracted.bin"), b"payload")
        });

        // Confirms the background thread has actually started before we
        // drop `opener` -- removes any ambiguity about ordering.
        started.wait();

        // No `std::mem::forget` here: this is what the pre-facade
        // `open_file_from_archive` implementation's code path would do
        // without its leak workaround, since `opener` is a plain local
        // binding that falls out of scope once its owning function
        // returns (immediately after spawning the background thread).
        drop(opener);

        // Lets the background thread's write attempt proceed now that
        // `opener` is gone.
        proceed.wait();
        let write_result = handle.join().expect("background thread must not panic");

        assert!(
            write_result.is_err() || !temp_dir.exists(),
            "BUG reproduced: FileOpener's Drop removes its temp directory the moment the \
             local `opener` binding goes out of scope, regardless of whether a background \
             thread (the real extraction, or the external application it later launches via \
             a non-blocking `explorer`/`xdg-open` spawn) still needs files inside it -- this \
             is exactly the hazard `std::mem::forget(opener)` used to work around by leaking \
             the directory forever instead of releasing it only once actually safe to do so."
        );
    }

    #[test]
    fn determine_extraction_strategy_treats_self_contained_media_as_file_only() {
        use super::{determine_extraction_strategy, OpenStrategy};
        assert_eq!(
            determine_extraction_strategy("folder/image.png"),
            OpenStrategy::FileOnly
        );
        assert_eq!(
            determine_extraction_strategy("folder/video.mp4"),
            OpenStrategy::FileOnly
        );
    }

    #[test]
    fn determine_extraction_strategy_treats_executables_as_same_directory() {
        use super::{determine_extraction_strategy, OpenStrategy};
        assert_eq!(
            determine_extraction_strategy("game/Game.exe"),
            OpenStrategy::SameDirectory
        );
    }

    #[test]
    fn parent_directory_and_basename_split_normalized_paths() {
        use super::{basename, parent_directory};
        assert_eq!(parent_directory("game/data/save.dat"), "game/data");
        assert_eq!(basename("game/data/save.dat"), "save.dat");
        assert_eq!(parent_directory("readme.txt"), "");
        assert_eq!(basename("readme.txt"), "readme.txt");
        // Backslash-separated paths normalize the same way.
        assert_eq!(parent_directory("game\\data\\save.dat"), "game/data");
        assert_eq!(basename("game\\data\\save.dat"), "save.dat");
    }
}
