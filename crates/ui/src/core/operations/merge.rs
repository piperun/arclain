//! Starting a split-archive merge through the application facade.
//!
//! Sibling of [`crate::core::operations::extraction`], and deliberately
//! the same shape: pick up what the dialog decided, open the tab's
//! progress dialog, dispatch `ArclainApp::start_merge`, and register the
//! resulting operation with `crate::core::operation_bridge` so its
//! progress, password challenges, and completion route back onto that
//! tab's signals. Nothing here waits on the merge.
//!
//! Pre-facade this all lived inline in
//! `crate::core::arclain_app::dialog_handler`'s merge-dialog arm, which
//! built an `arclain_core::services::MergeOptions` and called
//! `MergeService::merge` on a bare `runtime.spawn` -- see
//! `arclain_app::operations::merge`'s own module doc comment for what
//! that path could and could not do.

use std::sync::Arc;

use arclain_app::archive::MultiPartArchiveDto;
use arclain_app::operations::{MergeCompressionLevel, MergeOutputFormat, MergeRequest};

use crate::core::tabs::TabState;
use crate::shared::SharedState;

/// Starts merging `multipart` into a single archive on behalf of `tab`.
///
/// Fire-and-forget: returns as soon as the dispatch is spawned.
///
/// The merge occupies the tab's `active_extraction_operation` slot -- the
/// same slot an extraction uses -- because both drive the one per-tab
/// progress dialog. That is what makes the dialog's existing Cancel
/// button cancel a merge (`cancel_extraction` reads exactly this slot),
/// and what stops a merge and an extraction from overwriting each
/// other's progress fields, which the pre-facade merge (which tracked no
/// operation at all) allowed.
pub fn start_merge(
    shared: &SharedState,
    tab: &Arc<TabState>,
    multipart: MultiPartArchiveDto,
    output_format: MergeOutputFormat,
    compression_level: MergeCompressionLevel,
    delete_originals: bool,
) {
    // Guards on the tracked operation rather than the dialog's
    // visibility, for the reason `extraction::start_extraction`'s
    // identical guard spells out.
    if tab.active_extraction_operation.get().is_some() {
        shared.signals().status_bar.update(|status| {
            status.message = "Another archive operation is already running".to_string();
        });
        return;
    }
    let Some(app) = shared.facade.clone() else {
        tracing::error!("[merge] start_merge: no application facade available");
        return;
    };

    {
        // Built from `default()` rather than mutating whatever the last
        // operation left behind -- same reasoning as
        // `extraction::start_extraction`'s own dialog construction.
        let dialog = crate::shared::dialogs::ExtractionProgressDialog {
            show: true,
            title: "Merging Archive".to_string(),
            file_action: format!("Merging {} parts...", multipart.parts.len()),
            status: crate::shared::dialogs::ExtractionStatus::Running,
            // No facade-level pause/minimize primitive exists, only
            // cancellation -- the same reason extraction disables both.
            can_pause: false,
            can_minimize: false,
            can_cancel: true,
            started_at: Some(std::time::Instant::now()),
            ..Default::default()
        };
        tab.extraction_dialog().set(dialog);
    }

    shared.signals().status_bar.update(|status| {
        status.message = "Starting merge...".to_string();
    });

    let runtime = shared.services.tokio_runtime.clone();
    let shared = shared.clone();
    let tab_id = tab.id;
    let tab = tab.clone();
    runtime.spawn(async move {
        let request = MergeRequest {
            archive: multipart,
            output_format,
            compression_level,
            // The merge writes beside the set's own first part, named
            // after it -- the only destination this dialog has ever
            // offered, and the facade's own default for `None`.
            output_path: None,
            delete_originals,
            // No password up front: the dialog collects none, and an
            // encrypted set raises `Challenge::Password` through the
            // shared per-tab password dialog instead.
            password: None,
        };
        match app.start_merge(request).await {
            Ok(operation_id) => {
                // Set before registering, for the reason
                // `crate::core::operations::archive::start_archive_open`'s
                // own comment gives: `register_operation` reconciles
                // against the operation's current snapshot immediately,
                // and a fast-failing merge can already be terminal by
                // then.
                tab.active_extraction_operation.set(Some(operation_id));
                crate::core::operation_bridge::register_operation(&shared, operation_id, tab_id)
                    .await;
            }
            Err(error) => {
                tracing::error!("[merge] start_merge was rejected: {error:?}");
                let mut dialog = tab.extraction_dialog().get();
                dialog.show = false;
                dialog.status = crate::shared::dialogs::ExtractionStatus::Failed;
                tab.extraction_dialog().set(dialog);
                shared.signals().status_bar.update(|status| {
                    status.message = format!("Merge failed: {}", error.summary);
                });
            }
        }
    });
}
