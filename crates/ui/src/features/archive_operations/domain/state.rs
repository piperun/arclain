use crate::core::tabs::TabState;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

pub struct ArchiveOperationsState {
    // Note: extraction progress state (extraction_rx/extraction_child/
    // extraction_op_guard/extraction_origin_tab) is gone -- extraction
    // is now an application-facade operation (`arclain_app::operations::
    // extract`); the facade owns the CLI child process, and
    // `crate::core::operation_bridge` drives progress/completion onto
    // `TabState::extraction_dialog()`/`active_extraction_operation`
    // directly. See `crate::core::operations::extraction`.
    //
    // The conversion counterpart (conversion_rx/conversion_child/
    // conversion_started/conversion_op_guard/conversion_origin_tab) is
    // gone for the same reason, one step later: its only writer was the
    // pre-facade `convert_archive` bypass, which spawned the 7-Zip CLI
    // itself and pumped a `std::sync::mpsc` channel from the render
    // loop. Conversion is `ArclainApp::start_convert`/`start_pipeline`
    // now, and the Process page projects that operation's event stream
    // onto its own modal -- see
    // `crate::core::operations::process_runner`.
    /// Drag-out progress state. Carries the drag layer's own progress
    /// type (not core's `ProgressUpdate`): drag-out is facade-routed and
    /// its platform layer no longer speaks `arclain_core` types.
    pub drag_rx: Option<Receiver<crate::platform::drag_source::DragProgressUpdate>>,
    pub drag_started: Option<Instant>,
    /// The tab that originated this drag-out op. Captured at spawn time
    /// so `update_drag_progress` can route progress events to the
    /// originating tab's `drag_dialog` slot after the dialog migrated
    /// from `AppSignals.progress_dialogs` to `TabState.progress_dialogs`
    /// in the 2026-05-20 B3 reframed slice 2.
    pub drag_origin_tab: Option<Arc<TabState>>,
}

impl Default for ArchiveOperationsState {
    fn default() -> Self {
        Self {
            drag_rx: None,
            drag_started: None,
            drag_origin_tab: None,
        }
    }
}
