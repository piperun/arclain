//! Process page state — current pipeline, preview cache, run status, presets.

use arclain_core::{Pipeline, PipelinePreview, SavedPreset};

#[derive(Default)]
pub struct ProcessPageState {
    pub pipeline: Pipeline,
    /// Cached preview — rebuilt whenever pipeline changes
    pub preview: PipelinePreview,
    pub preview_dirty: bool,
    pub is_running: bool,
    pub last_result_summary: Option<String>,
    pub presets: Vec<SavedPreset>,
    pub active_preset_name: Option<String>,
    pub presets_path: Option<std::path::PathBuf>,
    /// Count of interrupted pipeline runs detected at app startup. Shown as
    /// a banner until the user dismisses it. `None` = not yet queried.
    pub interrupted_run_count: Option<usize>,
    pub interrupted_banner_dismissed: bool,
}

impl ProcessPageState {
    pub fn new() -> Self {
        let mut me = Self::default();
        me.load_presets();
        me
    }

    pub fn load_presets(&mut self) {
        self.presets_path = arclain_core::default_presets_path();
        if let Some(ref p) = self.presets_path {
            self.presets = arclain_core::load_presets(p);
        } else {
            self.presets = arclain_core::builtin_presets();
        }
    }

    pub fn save_presets(&self) {
        if let Some(ref p) = self.presets_path {
            if let Err(e) = arclain_core::save_presets(p, &self.presets) {
                tracing::error!("[process] failed to save presets: {}", e);
            }
        }
    }

    pub fn mark_dirty(&mut self) {
        self.preview_dirty = true;
    }

    pub fn refresh_preview(&mut self) {
        if self.preview_dirty {
            self.preview = arclain_core::preview_pipeline(&self.pipeline);
            self.preview_dirty = false;
        }
    }

    /// Lazily load the interrupted-run count from the DB on first access.
    /// The count surfaces a banner until the user dismisses it (session-local).
    pub fn ensure_interrupted_count(
        &mut self,
        config_db: Option<&std::sync::Arc<arclain_core::SqliteDb>>,
    ) {
        if self.interrupted_run_count.is_some() {
            return;
        }
        let count = match config_db {
            Some(db) => db
                .with_connection(|conn| {
                    Ok(arclain_core::list_interrupted_since(conn, 0)?.len())
                })
                .unwrap_or(0),
            None => 0,
        };
        self.interrupted_run_count = Some(count);
    }
}
