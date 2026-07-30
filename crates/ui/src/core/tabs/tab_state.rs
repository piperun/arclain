//! Per-tab state — owns the archive-context signals and the plugin pool.

use super::inventory::TabInventory;
use super::listing::TabListing;
use super::plugin_instances::TabPluginPool;
use super::view_state::{BrowserEntriesSnapshot, BrowserViewState};
use super::TabId;
use crate::core::operations::archive::{derive_archive_info, ArchiveExtras, ArchiveInfo};
use crate::core::signals::ToolbarContext;
use arclain_app::archive::ArchiveSnapshot;
use arclain_app::{Computed, Signal};
use arclain_core::features::organization::GameMetadata;
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::Arc;
use std::time::SystemTime;

/// One operation's pending challenge, as far as a tab's UI needs to know
/// to answer it. See [`TabState::pending_challenge`]'s own doc comment.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingChallenge {
    pub operation_id: arclain_app::ids::OperationId,
    pub challenge: arclain_app::challenge::Challenge,
}

pub struct TabState {
    pub id: TabId,

    // Per-tab archive-context signals (moved from AppSignals)
    pub archive_path: Signal<Option<PathBuf>>,
    /// Derived `archive_path.is_some()` — centralises the
    /// "is an archive loaded in this tab?" predicate that previously
    /// appeared as 6 duplicate `tab.archive_path.read().is_some()`
    /// checks across browser_page / app_rendering / toolbar_handler /
    /// update.rs (per the 2026-05-19 state-signals audit §3.9).
    /// Lazy on-read recompute via `Computed<T>::get` (no listener
    /// notifies — readers naturally pick up the new value the next
    /// frame because the renderer always polls each frame).
    pub archive_loaded: Computed<bool>,
    /// TRANSITIONAL(4b): every entry in this tab's archive at every
    /// depth, as the backend listed it.
    ///
    /// The archive browser's whole-archive consumers were built on this
    /// flat list: the folder tree's directory set, a drag-out's
    /// "everything under this folder" expansion, the derived
    /// [`ArchiveInfo`] totals, and the plugin bridge's event context.
    /// None of them are expressible through the application facade's
    /// read model yet -- `ArclainApp::list_entries` answers one
    /// *directory* at a time, and no facade method returns whole-archive
    /// [`arclain_app::archive::ArchiveEntryDto`] rows -- so the tab still
    /// holds the pre-facade shape for them, written by
    /// `crate::core::operation_bridge`'s own backend re-list.
    ///
    /// This is the field the browser-model migration deletes: the
    /// relisting side (4b) is what can page `list_entries` across the
    /// directory tree and hand the tab real, session-minted
    /// `EntryId`-carrying rows. Deliberately *not* synthesized into DTOs
    /// here in the meantime: an `EntryId` this crate invented would not
    /// be the one the owning session assigned, and every facade method
    /// that takes one (extract, delete, materialize) resolves it against
    /// that session -- a frontend-minted id could therefore name a
    /// different entry than the row the user picked.
    pub entries: Signal<Arc<Vec<arclain_core::ArchiveEntry>>>,
    /// Every entry in this tab's archive at every depth, as the facade's
    /// session reports it -- the [`TabInventory`] the operation bridge
    /// adopts from `ArclainApp::list_all_entries` on every relist.
    ///
    /// The whole-archive counterpart to [`Self::listing`] (which holds
    /// one directory's page), and the replacement for the flat
    /// [`Self::entries`] list above: rows here carry the session's own
    /// `EntryId`s and folder aggregates, and the not-yet-migrated
    /// core-typed consumers read the memoized `legacy_rows` projection
    /// instead of a separately-listed copy that could drift from what
    /// id-consuming operations resolve against.
    pub inventory: Signal<TabInventory>,
    pub metadata: Signal<Option<serde_json::Value>>,
    // Note: `loading` and `status_message` signals were removed in the
    // 2026-05-19 audit. Both were write-only — set by archive load /
    // tab switch but never read by any production code path. The
    // status bar carries the user-visible state instead.
    //
    // `ui_ready` (a "defer this tab's queued plugin event until the next
    // render frame" gate) was removed when the archive-open flow moved
    // onto `ArclainApp::start_open_archive`: the facade's own
    // `archive_ops::dispatch_archive_opened_event` fires the plugin event
    // directly from the operation worker, unconditionally, with no
    // frame-boundary gating needed -- the entire pending-queue mechanism
    // this flag gated (`AppState::pending_plugin_events`,
    // `Action::DispatchPendingPluginEvent`) became dead weight once
    // nothing produced it anymore.
    /// Backend-reported non-derivable archive metadata (encryption flags
    /// + encryption method). Source of truth for the encryption fields
    /// of `archive_info`; written once by
    /// `AppState::list_archive`/`list_with_password` after the backend
    /// `list` call returns.
    ///
    /// Split out from the old `archive_info: Signal<ArchiveInfo>` in
    /// the 2026-05-20 Tier 2 (item 6) audit: the other ArchiveInfo
    /// fields (counts, sizes, format, total_crc32) are derived from
    /// `entries` + `archive_path` by the `archive_info` Computed.
    pub archive_extras: Signal<ArchiveExtras>,
    /// Derived archive metadata — counts, sizes, format, total_crc32
    /// (from `entries` + `archive_path`) plus the encryption fields
    /// from `archive_extras`. Lazy on-read via `Computed<T>::get`.
    ///
    /// Pre 2026-05-20 Tier 2 item 6 this was a `Signal<ArchiveInfo>`
    /// written manually at 5 call sites (archive.rs, archive_ops.rs,
    /// toolbar_handler.rs, update.rs, browser_controller.rs). Making
    /// it Computed eliminates those writes; the derivation runs when
    /// any consumer (status bar render, properties panel) calls
    /// `.get()`. See audit §4.2 for why this was the right shape.
    pub archive_info: Computed<ArchiveInfo>,
    pub game_metadata: Signal<Option<GameMetadata>>,
    /// Where this tab is browsing inside its archive, the
    /// directory-scoped listing request that describes it, and the page
    /// the session answered with -- see [`TabListing`].
    ///
    /// Replaces the pre-facade `navigation:
    /// Signal<arclain_core::archive::NavigationState>`, which navigated a
    /// bare `String` cursor over the flat `entries` list above.
    ///
    /// The per-entry view state that has to survive a refresh -- selection,
    /// folder expansion, scroll -- is **not** keyed by the `EntryId` these
    /// listing rows carry; it stays keyed by archive-root path.
    /// [`BrowserViewState`] documents each one and why re-keying belongs
    /// with the render-side migration rather than here.
    pub listing: Signal<TabListing>,
    pub current_password: Signal<Option<String>>,
    pub selection_count: Signal<usize>,
    /// TRANSITIONAL(4c): the direct backend handle a drag-out extraction
    /// still reads `backend_arc()`/`password_ref()` off
    /// (`crate::features::archive_browser::application::drag_drop_service`).
    /// Every other archive I/O path goes through the facade's own session
    /// (see [`Self::archive_session_id`]); the render-side consumers are
    /// what still need this one, so it dies when they move.
    pub opened_archive: Signal<Option<Arc<RwLock<arclain_core::Archive>>>>,
    /// The application facade's session id for this tab's open archive
    /// (`None` when no archive is open in this tab). Set once the
    /// `start_open_archive` operation reaches `Completed { ArchiveOpened }`
    /// (see `crate::core::operations::archive`); cleared when the tab's
    /// archive is closed/replaced.
    ///
    /// `arclain_app`'s per-session archive read model
    /// (`ArclainApp::list_entries`/`archive_snapshot`) is keyed by this id,
    /// which a tab uses as the stable handle to reach it. `opened_archive`
    /// above still carries the direct backend handle a few not-yet-
    /// migrated call sites need (drag-out extraction in particular reads
    /// `backend_arc()`/`password_ref()` off it directly) -- both fields
    /// are populated together by `crate::core::operation_bridge` once
    /// `start_open_archive` completes (see its own doc comment for why
    /// that means a second, UI-owned `list()` call rather than reaching
    /// into the facade's own indexed session).
    pub archive_session_id: Signal<Option<arclain_app::ids::ArchiveSessionId>>,
    /// The [`ArchiveSnapshot`] the facade reported for
    /// [`Self::archive_session_id`]'s session -- its revision, source
    /// path, archive type, aggregate entry count and uncompressed total,
    /// and whatever metadata a plugin has written for it.
    ///
    /// Written by `crate::core::operation_bridge` from the very
    /// `OperationResult::ArchiveOpened { snapshot }` that stamps the
    /// session id above, so the two never disagree about which archive a
    /// tab holds. `None` until this tab's first successful open.
    ///
    /// Not yet what [`Self::archive_info`] derives from: a snapshot
    /// reports no compressed total and no archive-wide CRC-32, and its
    /// `entry_count` includes the ancestor directories the session's
    /// index synthesizes (the pre-facade count is the backend's own row
    /// count). Closing those gaps is a facade-side change, so the
    /// derivation still reads the flat [`Self::entries`] list.
    pub archive_snapshot: Signal<Option<ArchiveSnapshot>>,
    /// Every operation (archive-open, extraction, or both at once --
    /// see below) currently awaiting a response to a challenge it
    /// raised, oldest first. The front entry is the one shown via this
    /// tab's `password_dialog`; the rest wait their turn.
    ///
    /// A single `Option` slot held only the most recent challenge until
    /// the 2026-07 operation-bridge hardening: a tab's archive-open and
    /// its extraction are independent operations that can both be in
    /// flight at once (e.g. the user starts extracting, then opens a
    /// different archive into the same tab before the extraction
    /// finishes), and either can raise its own `Challenge::Password`.
    /// With one slot, the second challenge to arrive silently overwrote
    /// the first -- the first operation's challenge waiter then hung
    /// forever, since nothing would ever answer it. A small FIFO queue
    /// means every challenge eventually gets shown, never dropped.
    ///
    /// Populated by `crate::core::operation_bridge` when a `Challenge`
    /// arrives for an operation this tab originated; the front entry is
    /// removed (and the next, if any, presented) via
    /// `crate::core::operation_bridge::dequeue_and_present_next` once
    /// answered, cancelled, or the operation reaches a terminal state.
    /// The render side reads the front entry to know which
    /// `OperationId`/`ChallengeId` a submitted answer targets, rather
    /// than assuming "whatever `try_open_with_password` used to call"
    /// the way the pre-facade UI could.
    pub pending_challenge: Signal<Vec<PendingChallenge>>,
    /// The extraction operation (if any) currently running for this tab
    /// -- set by `crate::features::archive_operations::application::
    /// extraction::start_extraction` right after registering with the
    /// operation bridge, cleared by the bridge once the operation
    /// reaches a terminal state. What the extraction dialog's Cancel
    /// button targets: the facade owns the CLI child process now, so
    /// cancelling means `ArclainApp::cancel_operation`, not killing a
    /// handle egui holds directly.
    pub active_extraction_operation: Signal<Option<arclain_app::ids::OperationId>>,
    /// The archive-open operation (if any) currently running for this
    /// tab -- set by `crate::core::operations::archive::start_archive_open`
    /// right after registering with the operation bridge, cleared once
    /// the operation reaches a terminal state. Mirrors
    /// `active_extraction_operation`'s role for extraction: what a
    /// tab-close must cancel before the tab goes away (see
    /// `crate::core::operations::archive::cancel_archive_open` and its
    /// call site in the close-tab-confirm handler) so an open in flight
    /// for a tab that no longer exists doesn't keep running orphaned.
    pub pending_open_operation: Signal<Option<arclain_app::ids::OperationId>>,
    /// Worker-owned immutable file-list snapshot. Renderers may clone this
    /// signal value in O(1), but only archive/navigation workers replace it.
    pub browser_entries: Signal<BrowserEntriesSnapshot>,
    pub browser_view_state: Signal<BrowserViewState>,
    /// Monotonic request key for text reads in this tab. Only the newest
    /// matching request may publish into `file_edit_dialog`.
    pub file_request_seq: AtomicU64,
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
    /// Password-dialog state for unlocking encrypted archives.
    /// Migrated from the global `AppSignals.password_dialog` in the
    /// 2026-05-20 B3 reframed slice — the prompt is bound to the
    /// archive being opened (and therefore the tab loading it).
    /// Two encrypted archives in two tabs no longer overwrite each
    /// other's prompt; closing a tab drops its pending request.
    /// The pre-migration `pending_tab_id` cross-tab routing field
    /// is gone — the dialog living on a tab is the implicit routing.
    pub password_dialog: Signal<crate::features::password_management::dialogs::PasswordDialog>,
    /// Auto-retry token for "click file → extract → password failure
    /// → user enters password → re-fire the same file open".
    /// `process_extraction_progress` sets this from
    /// `progress.requested_file_path` when an extraction fails with
    /// a password error. `dialog_handler` reads + clears it on the
    /// successful-unlock branch and writes `pending_open_file` to
    /// re-trigger the file-open flow with `current_password` now
    /// populated. None on success or non-password failures, so the
    /// signal stays empty for routes that don't need it.
    pub pending_open_after_unlock: Signal<Option<String>>,
    /// Progress-dialog state for the three long-running op flavours
    /// (extraction / conversion / drag-out). Migrated from the global
    /// `AppSignals.progress_dialogs` in the 2026-05-20 B3 reframed
    /// slice — the dialog visualises an op that always originates on
    /// a specific tab (the one that owns the archive). Closing the
    /// tab during an in-flight op drops the dialog with it; the
    /// background worker still kills its subprocess via `tab_cancel`.
    /// The A3 slot-struct + proxy pattern (commit 9975481) is
    /// preserved — only the field's location moves. Access via the
    /// `extraction_dialog()` / `conversion_dialog()` / `drag_dialog()`
    /// accessors below.
    pub progress_dialogs: Signal<crate::shared::dialogs::ProgressDialogs>,

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

    /// Set true once this tab's signals have been subscribed to the
    /// egui ctx-repaint. Guards `bind_to_context_once` against
    /// installing duplicate listeners. Without this, every per-frame
    /// sweep / every `signals.tabs.set` would stack another listener
    /// onto every signal and the same write would notify 2x, 3x, …
    pub signals_bound: AtomicBool,

    // Plugin instance pool (Phase 2c populates)
    pub plugin_pool: TabPluginPool,
}

impl TabState {
    pub fn new(id: TabId) -> Self {
        let archive_path: Signal<Option<PathBuf>> = Signal::new(None).with_name("archive_path");
        let entries: Signal<Arc<Vec<arclain_core::ArchiveEntry>>> =
            Signal::new(Arc::new(Vec::new())).with_name("entries");
        let archive_extras: Signal<ArchiveExtras> =
            Signal::new(ArchiveExtras::default()).with_name("archive_extras");
        let archive_loaded = {
            let archive_path = archive_path.clone();
            Computed::new(move || archive_path.read().is_some())
        };
        let archive_info = {
            let entries = entries.clone();
            let archive_path = archive_path.clone();
            let archive_extras = archive_extras.clone();
            Computed::new(move || {
                let ents = entries.get();
                let path = archive_path.get();
                let extras = archive_extras.get();
                derive_archive_info(ents.as_slice(), path.as_deref(), &extras)
            })
        };
        Self {
            id,
            archive_path,
            archive_loaded,
            entries,
            inventory: Signal::new(TabInventory::default()).with_name("inventory"),
            metadata: Signal::new(None).with_name("metadata"),
            archive_extras,
            archive_info,
            game_metadata: Signal::new(None).with_name("game_metadata"),
            listing: Signal::new(TabListing::default()).with_name("listing"),
            current_password: Signal::new(None).with_name("current_password"),
            selection_count: Signal::new(0).with_name("selection_count"),
            opened_archive: Signal::new(None).with_name("opened_archive"),
            archive_session_id: Signal::new(None).with_name("archive_session_id"),
            archive_snapshot: Signal::new(None).with_name("archive_snapshot"),
            pending_challenge: Signal::new(Vec::new()).with_name("pending_challenge"),
            active_extraction_operation: Signal::new(None).with_name("active_extraction_operation"),
            pending_open_operation: Signal::new(None).with_name("pending_open_operation"),
            browser_entries: Signal::new(BrowserEntriesSnapshot::default())
                .with_name("browser_entries"),
            browser_view_state: Signal::new(BrowserViewState::default())
                .with_name("browser_view_state"),
            file_request_seq: AtomicU64::new(0),
            page_display_name: Signal::new(None).with_name("page_display_name"),
            active_toolbar: Signal::new(ToolbarContext::Archive).with_name("active_toolbar"),
            pending_open_file: Signal::new(None).with_name("pending_open_file"),
            file_edit_dialog: Signal::new(crate::features::file_editing::FileEditDialog::default())
                .with_name("file_edit_dialog"),
            merge_dialog: Signal::new(crate::shared::dialogs::MergeDialogState::default())
                .with_name("merge_dialog"),
            lightbox_state: Signal::new(crate::shared::dialogs::LightboxState::default())
                .with_name("lightbox_state"),
            password_dialog: Signal::new(
                crate::features::password_management::dialogs::PasswordDialog::default(),
            )
            .with_name("password_dialog"),
            pending_open_after_unlock: Signal::new(None).with_name("pending_open_after_unlock"),
            progress_dialogs: Signal::new(crate::shared::dialogs::ProgressDialogs::default())
                .with_name("progress_dialogs"),
            created_at: SystemTime::now(),
            in_flight_ops: Arc::new(AtomicUsize::new(0)),
            tab_cancel: Arc::new(AtomicBool::new(false)),
            pinned: Arc::new(AtomicBool::new(false)),
            signals_bound: AtomicBool::new(false),
            plugin_pool: TabPluginPool::default(),
        }
    }

    /// Subscribe every per-tab Signal to egui ctx-repaint. Idempotent
    /// via the `signals_bound` AtomicBool flag — calling this more
    /// than once on the same TabState early-returns on the second
    /// call so we don't stack duplicate listeners.
    ///
    /// The full repaint-binding story: `AppSignals::bind_to_context`
    /// (called once from `bind_signals_once` on the first frame)
    /// binds the outer `tabs` collection signal AND subscribes a
    /// "bind any newly-added tabs" sweep to it. That sweep iterates
    /// the post-set collection and calls this method on each tab.
    /// New tabs (drop overlay, Ctrl+T, reopen-closed, persistence
    /// restore) therefore get their per-tab signals bound the moment
    /// they join the collection. Background writes to per-tab signals
    /// from worker threads (archive list, extraction progress, etc.)
    /// then trigger UI repaints automatically, no manual
    /// `ctx.request_repaint()` needed at the write sites.
    ///
    /// Note: `archive_loaded` / `archive_info` are `Computed`, not
    /// `Signal`, and recompute lazily on `.get()` — readers pick up the
    /// new value the next frame because the renderer always polls each
    /// frame.
    pub fn bind_to_context_once(&self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        if self.signals_bound.swap(true, Ordering::SeqCst) {
            return;
        }
        let sig_ctx = crate::core::signal_context::SignalContext::new(ctx.clone());
        sig_ctx.bind_named(&self.archive_path, "tab.archive_path");
        sig_ctx.bind_named(&self.entries, "tab.entries");
        sig_ctx.bind_named(&self.inventory, "tab.inventory");
        sig_ctx.bind_named(&self.metadata, "tab.metadata");
        sig_ctx.bind_named(&self.archive_extras, "tab.archive_extras");
        sig_ctx.bind_named(&self.game_metadata, "tab.game_metadata");
        sig_ctx.bind_named(&self.listing, "tab.listing");
        sig_ctx.bind_named(&self.current_password, "tab.current_password");
        sig_ctx.bind_named(&self.selection_count, "tab.selection_count");
        sig_ctx.bind_named(&self.opened_archive, "tab.opened_archive");
        sig_ctx.bind_named(&self.archive_session_id, "tab.archive_session_id");
        sig_ctx.bind_named(&self.archive_snapshot, "tab.archive_snapshot");
        sig_ctx.bind_named(&self.pending_challenge, "tab.pending_challenge");
        sig_ctx.bind_named(
            &self.active_extraction_operation,
            "tab.active_extraction_operation",
        );
        sig_ctx.bind_named(&self.pending_open_operation, "tab.pending_open_operation");
        sig_ctx.bind_named(&self.browser_entries, "tab.browser_entries");
        sig_ctx.bind_named(&self.browser_view_state, "tab.browser_view_state");
        sig_ctx.bind_named(&self.page_display_name, "tab.page_display_name");
        sig_ctx.bind_named(&self.active_toolbar, "tab.active_toolbar");
        sig_ctx.bind_named(&self.pending_open_file, "tab.pending_open_file");
        sig_ctx.bind_named(&self.file_edit_dialog, "tab.file_edit_dialog");
        sig_ctx.bind_named(&self.merge_dialog, "tab.merge_dialog");
        sig_ctx.bind_named(&self.lightbox_state, "tab.lightbox_state");
        sig_ctx.bind_named(&self.password_dialog, "tab.password_dialog");
        sig_ctx.bind_named(
            &self.pending_open_after_unlock,
            "tab.pending_open_after_unlock",
        );
        sig_ctx.bind_named(&self.progress_dialogs, "tab.progress_dialogs");
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

    /// Proxy for this tab's extraction progress-dialog slot. Mirrors
    /// the pre-2026-05-20 `AppSignals::extraction_dialog()` shape
    /// (`.get()`, `.set()`, `.set_if_changed()`) so callsites only
    /// changed their navigation (active or origin tab) — not the
    /// proxy chain. See the slot-struct rationale in
    /// `shared::dialogs::progress::ProgressDialogs`.
    pub fn extraction_dialog(&self) -> crate::core::signals::ProgressDialogProxy<'_> {
        crate::core::signals::ProgressDialogProxy::extraction(&self.progress_dialogs)
    }

    /// Proxy for this tab's conversion progress-dialog slot. See
    /// [`Self::extraction_dialog`].
    pub fn conversion_dialog(&self) -> crate::core::signals::ProgressDialogProxy<'_> {
        crate::core::signals::ProgressDialogProxy::conversion(&self.progress_dialogs)
    }

    /// Proxy for this tab's drag-out progress-dialog slot. See
    /// [`Self::extraction_dialog`].
    pub fn drag_dialog(&self) -> crate::core::signals::ProgressDialogProxy<'_> {
        crate::core::signals::ProgressDialogProxy::drag(&self.progress_dialogs)
    }
}

#[cfg(test)]
#[path = "tab_state_tests.rs"]
mod tests;
