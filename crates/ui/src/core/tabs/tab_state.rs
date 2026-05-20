//! Per-tab state — owns the archive-context signals and the plugin pool.

use super::plugin_instances::TabPluginPool;
use super::TabId;
use crate::core::operations::archive::ArchiveInfo;
use crate::core::signals::ToolbarContext;
use crate::features::archive_browser::domain::types::BrowserViewState;
use arclain_core::archive::NavigationState;
use arclain_core::features::organization::GameMetadata;
use arclain_core::ArchiveEntry;
use arclain_signals::Signal;
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::SystemTime;

pub struct TabState {
    pub id: TabId,

    // Per-tab archive-context signals (moved from AppSignals)
    pub archive_path: Signal<Option<PathBuf>>,
    pub entries: Signal<Arc<Vec<ArchiveEntry>>>,
    pub metadata: Signal<Option<serde_json::Value>>,
    // Note: `loading` and `status_message` signals were removed in the
    // 2026-05-19 audit. Both were write-only — set by archive load /
    // tab switch but never read by any production code path. The
    // status bar carries the user-visible state instead.
    pub ui_ready: Signal<bool>,
    pub archive_info: Signal<ArchiveInfo>,
    pub game_metadata: Signal<Option<GameMetadata>>,
    pub navigation: Signal<NavigationState>,
    pub current_password: Signal<Option<String>>,
    pub selection_count: Signal<usize>,
    pub opened_archive: Signal<Option<Arc<RwLock<arclain_core::Archive>>>>,
    pub browser_view_state: Signal<BrowserViewState>,
    pub page_display_name: Signal<Option<String>>,
    pub active_toolbar: Signal<ToolbarContext>,
    /// When a file is double-clicked in this tab's archive list, the
    /// path lands here for the main loop to pick up and process.
    /// Migrated from a global `AppSignals.pending_open_file` in the
    /// 2026-05-19 audit: the "file to open next" is inherently
    /// per-tab — closing the tab cleanly drops the pending request.
    pub pending_open_file: Signal<Option<String>>,
    /// File-edit dialog state for this tab. Migrated from the global
    /// `AppSignals.file_edit_dialog` in the 2026-05-19 audit — the
    /// edit operation is bound to a specific archive (and therefore
    /// tab); closing the tab during an edit should close the dialog.
    pub file_edit_dialog: Signal<crate::features::file_editing::FileEditDialog>,
    /// Merge-dialog state for merging split archives. Migrated from
    /// the global `AppSignals.merge_dialog` in the 2026-05-20 audit
    /// B2 follow-up — the merge operation always targets the tab's
    /// active archive, so closing the tab while a merge dialog is
    /// open should drop the dialog state.
    pub merge_dialog: Signal<crate::shared::dialogs::MergeDialogState>,
    /// Lightbox state for the full-screen image viewer. Migrated
    /// from the global `AppSignals.lightbox_state` in the 2026-05-20
    /// audit B2 follow-up — the lightbox is plugin-driven, and the
    /// plugin is tied to a tab. Switching tabs hides the lightbox
    /// naturally; switching back restores it.
    pub lightbox_state: Signal<crate::shared::dialogs::LightboxState>,

    // Tab metadata (not signals — read on render)
    pub created_at: SystemTime,
    pub in_flight_ops: Arc<AtomicUsize>,

    /// Cooperative cancellation flag. Fired by `TabsCollection::force_close`
    /// when the user confirms closing a tab that has in-flight ops. Long-
    /// running ops (extraction, conversion, plugin calls, etc.) should
    /// periodically check this flag and abort + clean up partial output
    /// when set, per the ACID contract documented in the Phase 2 design
    /// spec.
    ///
    /// v1 is best-effort: not all op types check the flag yet. A future
    /// audit pass migrates each spawn site. Ops that ignore the flag
    /// continue against the captured `Arc<TabState>` until completion;
    /// the tab is already removed from the collection so the user can't
    /// see them, but they keep consuming resources until done.
    pub tab_cancel: Arc<AtomicBool>,

    /// Pinned tabs render with a pin glyph and are kept at the front
    /// of the collection. They're excluded from `Close other` and
    /// `Close to the right` bulk actions and from middle-click close
    /// (matches the browser-tab convention — pinned = "I want this
    /// to stick around"). Atomic so background ops can read it
    /// without locking the signal.
    pub pinned: Arc<AtomicBool>,

    // Plugin instance pool (Phase 2c populates)
    pub plugin_pool: TabPluginPool,
}

impl TabState {
    pub fn new(id: TabId) -> Self {
        Self {
            id,
            archive_path: Signal::new(None).with_name("archive_path"),
            entries: Signal::new(Arc::new(Vec::new())).with_name("entries"),
            metadata: Signal::new(None).with_name("metadata"),
            ui_ready: Signal::new(true).with_name("ui_ready"),
            archive_info: Signal::new(ArchiveInfo::default()).with_name("archive_info"),
            game_metadata: Signal::new(None).with_name("game_metadata"),
            navigation: Signal::new(NavigationState::new()).with_name("navigation"),
            current_password: Signal::new(None).with_name("current_password"),
            selection_count: Signal::new(0).with_name("selection_count"),
            opened_archive: Signal::new(None).with_name("opened_archive"),
            browser_view_state: Signal::new(BrowserViewState::default())
                .with_name("browser_view_state"),
            page_display_name: Signal::new(None).with_name("page_display_name"),
            active_toolbar: Signal::new(ToolbarContext::Archive).with_name("active_toolbar"),
            pending_open_file: Signal::new(None).with_name("pending_open_file"),
            file_edit_dialog: Signal::new(
                crate::features::file_editing::FileEditDialog::default(),
            )
            .with_name("file_edit_dialog"),
            merge_dialog: Signal::new(crate::shared::dialogs::MergeDialogState::default())
                .with_name("merge_dialog"),
            lightbox_state: Signal::new(crate::shared::dialogs::LightboxState::default())
                .with_name("lightbox_state"),
            created_at: SystemTime::now(),
            in_flight_ops: Arc::new(AtomicUsize::new(0)),
            tab_cancel: Arc::new(AtomicBool::new(false)),
            pinned: Arc::new(AtomicBool::new(false)),
            plugin_pool: TabPluginPool::default(),
        }
    }

    /// Display title derived from the current archive_path. Recomputed
    /// on every call — cheap, and avoids signal-on-signal complexity.
    pub fn display_title(&self) -> String {
        match self.archive_path.get() {
            Some(path) => path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string(),
            None => "New tab".to_string(),
        }
    }
}

#[cfg(test)]
#[path = "tab_state_tests.rs"]
mod tests;
