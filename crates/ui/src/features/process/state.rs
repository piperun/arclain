//! Process page state — current pipeline, preview cache, run status.

use arclain_core::{Pipeline, PipelinePreview};

#[derive(Default)]
pub struct ProcessPageState {
    pub pipeline: Pipeline,
    /// Cached preview — rebuilt whenever pipeline changes
    pub preview: PipelinePreview,
    pub preview_dirty: bool,
    pub is_running: bool,
    pub last_result_summary: Option<String>,
}

impl ProcessPageState {
    pub fn mark_dirty(&mut self) {
        self.preview_dirty = true;
    }

    pub fn refresh_preview(&mut self) {
        if self.preview_dirty {
            self.preview = arclain_core::preview_pipeline(&self.pipeline);
            self.preview_dirty = false;
        }
    }
}
