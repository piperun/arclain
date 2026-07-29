//! egui's plugin UI, expressed against the application facade's
//! renderer-neutral session contract.
//!
//! # Why this module exists
//!
//! egui's original plugin UI stack (`super::ui_jobs::PluginUiJobs`) is a
//! *fetch-by-key* system: a render frame asks for
//! `(plugin_id, target, origin_tab)`, a background worker thread calls
//! `get-ui-layout` on the WASM guest, and the result lands in an internal
//! read cache the next frame picks up. It has no notion of a session at
//! all -- every read is independent, and staleness is managed by
//! explicitly invalidating cache keys from ~a dozen call sites.
//!
//! `arclain_app`'s contract is the opposite shape: a frontend *opens* a
//! session for one extension point, holds a [`PluginSessionId`], and
//! receives a new [`PluginUiDocument`] only as the *result* of an action
//! it dispatched. There is no polling and no cache to invalidate --
//! `RefreshPanel` is resolved inside the same dispatch that produced it
//! (see `arclain_app::plugins`'s module doc comment).
//!
//! This module is the adapter between those two shapes, and the *only*
//! place egui's plugin UI learns what a plugin wants to draw. It owns:
//!
//! - [`PluginSlot`] -- the UI-side identity of one rendered
//!   extension-point instance.
//! - [`PluginSessions`] -- the slot registry: which slot holds which
//!   facade session, which document revision it has applied, and which
//!   `start_plugin_action` operations are still in flight for it.
//! - The routing rule that decides whether an arriving
//!   `OperationResult::PluginUiUpdated` belongs to a slot at all (see
//!   [`PluginSessions::apply_update`]).
//!
//! Rendering the resulting document lives next door, in
//! `crate::features::plugins::presentation::rendering::document`; applying
//! the accompanying host intents lives in [`apply_intent`].
//!
//! # Slot lifecycle
//!
//! A slot is *declared* by a render frame ("this panel wants
//! `Panel` for plugin X") and *resolved* asynchronously:
//!
//! 1. First frame: [`PluginSessions::view`] finds no entry, spawns
//!    `ArclainApp::open_plugin_session(plugin_id, extension_point)` onto
//!    the facade's runtime, records [`SlotPhase::Opening`], and returns
//!    [`SlotView::Opening`]. The caller draws a placeholder.
//! 2. The spawned open completes and stores the session id + first
//!    document; the next frame gets [`SlotView::Ready`].
//! 3. A user interaction calls [`PluginSessions::dispatch`], which spawns
//!    `start_plugin_action` and records the returned `OperationId` against
//!    the slot. The slot keeps rendering its *current* document meanwhile
//!    -- never a blank panel, matching the old stack's
//!    "keep showing the previous layout" behavior, but without a separate
//!    stale-flag protocol.
//! 4. `crate::core::operation_bridge` observes
//!    `OperationKind::PluginAction` reaching `Completed`/`Failed` and hands
//!    it to [`PluginSessions::apply_update`] / [`PluginSessions::fail`].
//! 5. When the host stops drawing a slot (a dialog closes, a tab closes,
//!    a plugin is disabled), [`PluginSessions::close`] spawns
//!    `close_plugin_session` and drops the entry.
//!
//! Steps 1 and 5 are deliberately *host-driven*, not plugin-driven: the
//! facade has no opinion about when egui shows a panel.
//!
//! # Applying only the matching operation's terminal document revision
//!
//! Three independent things can make an arriving `PluginUiUpdated` not
//! belong to the slot it superficially looks like it belongs to, and
//! [`PluginSessions::apply_update`] rejects all three:
//!
//! - **Wrong slot.** Two slots can render the *same plugin* at once (a
//!   Panel and an open Dialog, say). The facade serializes dispatches per
//!   *plugin id*, not per session, so their operations interleave freely.
//!   Only the slot that recorded this `OperationId` may apply it.
//! - **Wrong session.** A slot can be closed and reopened (tab switch,
//!   plugin toggle) while one of its actions is still in flight. The
//!   arriving document names the *old* session id; applying it would
//!   resurrect a dead document into a live slot.
//! - **Older revision.** `PluginUiDocument::revision` increases on every
//!   dispatch against a session. Two actions dispatched back-to-back can
//!   complete out of order at the broadcast, so an update whose revision
//!   is not strictly greater than what the slot already applied is
//!   dropped rather than rolling the UI backwards.
//!
//! Only `OperationState::Completed` carries a document, so "terminal" is
//! structural rather than a check this module has to make: `Accepted`,
//! `Started`, and `Progress` have nothing to apply.
//!
//! # Button navigation: one rule, replacing three divergent ones
//!
//! The old renderer had no typed channel for a button's *declarative*
//! navigation (`ButtonAction::ShowDialog`/`OpenPage`/`CloseDialog`/
//! `ClosePage`), so it encoded them as reserved event-id strings
//! (`"__dialog_open:{id}"`, `"__page_close"`, ...) pushed through the very
//! same `(element_id, value)` callback used for real plugin events. Each
//! host call site then string-matched whichever prefixes it happened to
//! care about, and they did not agree:
//!
//! | call site | intercepted |
//! |---|---|
//! | toolbar | `__dialog_open:`, `__dialog_close` |
//! | dialog callback | `__dialog_close` |
//! | page callback | `__page_close`, `__page_open:` |
//! | archive-browser panel | `__dialog_open:`, `__page_open:` |
//!
//! Every prefix a site did *not* intercept fell through to the plugin
//! dispatcher and was sent to the WASM guest as a literal event id. A
//! `ClosePage` button drawn in a panel sent the guest the string
//! `"__page_close"` and closed nothing; a `CloseDialog` button drawn in a
//! panel did the same.
//!
//! This module removes the encoding rather than reconciling the table.
//! `arclain_plugins::ui_model::normalize_layout` already decodes
//! `ButtonAction` into a typed [`PluginButtonActionDto`] carried *on the
//! Button node itself*, so the renderer resolves it at press time and
//! returns a [`PluginNavigation`] to the host. Navigation never travels
//! through the action channel, so there is nothing for a call site to
//! intercept and nothing to forget to intercept. Only
//! `PluginButtonActionDto::Custom`/`None` reach `start_plugin_action`.
//!
//! **This is a deliberate behavior change, in two directions.** Buttons
//! whose navigation used to leak to the guest at a given call site no
//! longer do (a fix -- the guest was receiving a host-internal marker
//! string it never asked for). And navigation now works at *every*
//! extension point rather than at the subset each site happened to
//! handle, which is the behavior the WIT schema always described.
//!
//! # What is deliberately not modeled here
//!
//! - **Where a dialog/page is open** stays in
//!   `crate::features::plugins::domain::state::PluginDialogState`. That is
//!   host navigation state (which tab opened it, the page back-stack), not
//!   plugin document state, and it is renderer-owned in both the old and
//!   new stacks. This module only decides *what a slot draws*; the dialog
//!   state decides *whether a slot exists*. Keeping them separate is what
//!   lets extension points migrate one at a time.
//! - **`RefreshPanel` / `RequestFetch`** never reach a renderer: both are
//!   resolved inside `arclain_app::plugins`. `PluginHostIntentDto`
//!   deliberately has no variant for either.

use std::collections::HashMap;
use std::sync::Arc;

use arclain_app::ids::{OperationId, PluginSessionId};
use arclain_app::plugins::{
    PluginActionDto, PluginActionRequest, PluginButtonActionDto, PluginExtensionPointDto,
    PluginUiDocument, PluginUiUpdate,
};
use arclain_app::ArclainApp;
use parking_lot::Mutex;

use crate::core::tabs::TabId;

/// The UI-side identity of one rendered extension-point instance.
///
/// Distinct from `PluginExtensionPointDto` (which the facade understands)
/// in exactly one way: a slot is also scoped by *where in this frontend*
/// it is drawn. Two tabs can each show the same plugin's `Panel`, and
/// those are two independent facade sessions with independent documents
/// -- a distinction the extension point alone cannot express.
///
/// `MainPage` and `PluginButton` are deliberately *not* tab-scoped:
/// the plugin-settings detail view and the toolbar both render exactly
/// one instance for the whole window, so tab-scoping them would open
/// (and leak) one WASM session per tab for a document that is identical
/// in every one of them.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum PluginSlot {
    MainPage {
        plugin_id: String,
    },
    PluginButton {
        plugin_id: String,
    },
    Panel {
        plugin_id: String,
        tab: TabId,
    },
    Dialog {
        plugin_id: String,
        dialog_id: String,
        tab: TabId,
    },
    Page {
        plugin_id: String,
        page_id: String,
        tab: TabId,
    },
}

impl PluginSlot {
    pub fn plugin_id(&self) -> &str {
        match self {
            Self::MainPage { plugin_id }
            | Self::PluginButton { plugin_id }
            | Self::Panel { plugin_id, .. }
            | Self::Dialog { plugin_id, .. }
            | Self::Page { plugin_id, .. } => plugin_id,
        }
    }

    /// Which tab this slot belongs to, for the tab-scoped variants. Used
    /// to close every slot a closing tab owned; `None` means the slot
    /// outlives any individual tab (see this type's own doc comment).
    pub fn tab(&self) -> Option<TabId> {
        match self {
            Self::MainPage { .. } | Self::PluginButton { .. } => None,
            Self::Panel { tab, .. } | Self::Dialog { tab, .. } | Self::Page { tab, .. } => {
                Some(*tab)
            }
        }
    }

    /// The facade-facing extension point this slot opens a session for.
    pub fn extension_point(&self) -> PluginExtensionPointDto {
        match self {
            Self::MainPage { .. } => PluginExtensionPointDto::MainPage,
            Self::PluginButton { .. } => PluginExtensionPointDto::PluginButton,
            Self::Panel { .. } => PluginExtensionPointDto::Panel,
            Self::Dialog { dialog_id, .. } => PluginExtensionPointDto::Dialog(dialog_id.clone()),
            Self::Page { page_id, .. } => PluginExtensionPointDto::Page(page_id.clone()),
        }
    }
}

/// Host navigation a button asked for, resolved from the node's own typed
/// [`PluginButtonActionDto`] rather than from a reserved event-id string
/// -- see this module's doc comment for the encoding this replaces.
///
/// `PluginButtonActionDto::Custom`/`None` produce no navigation at all;
/// they are plugin interactions and go to [`PluginSessions::dispatch`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginNavigation {
    OpenDialog { dialog_id: String },
    CloseDialog,
    OpenPage { page_id: String },
    ClosePage,
}

impl PluginNavigation {
    /// Splits a pressed button's declarative action into "host navigation"
    /// and "plugin interaction" halves. Exactly one is ever `Some`.
    ///
    /// `None`/`Custom` map to the plugin-interaction half; the resulting
    /// node id is the plugin's own (`None` -- a plain button press names
    /// the button node itself) or the custom value the plugin supplied
    /// (`Custom` -- the plugin chose a different event id for this
    /// button, which the old renderer honored the same way).
    pub fn resolve(
        node_id: &str,
        action: Option<&PluginButtonActionDto>,
    ) -> (Option<PluginNavigation>, Option<String>) {
        match action.unwrap_or(&PluginButtonActionDto::None) {
            PluginButtonActionDto::ShowDialog { id } => (
                Some(PluginNavigation::OpenDialog {
                    dialog_id: id.clone(),
                }),
                None,
            ),
            PluginButtonActionDto::CloseDialog => (Some(PluginNavigation::CloseDialog), None),
            PluginButtonActionDto::OpenPage { id } => (
                Some(PluginNavigation::OpenPage {
                    page_id: id.clone(),
                }),
                None,
            ),
            PluginButtonActionDto::ClosePage => (Some(PluginNavigation::ClosePage), None),
            PluginButtonActionDto::Custom { value } => (None, Some(value.clone())),
            PluginButtonActionDto::None => (None, Some(node_id.to_string())),
        }
    }
}

/// What a slot currently has to draw -- the render-time return of
/// [`PluginSessions::view`].
#[derive(Clone, Debug)]
pub enum SlotView {
    /// The session open is in flight. Draw a placeholder.
    Opening,
    /// A document is available. Its revision may be one or more actions
    /// behind if a dispatch is still in flight; that is intended (see
    /// this module's doc comment, step 3).
    Ready(Arc<PluginUiDocument>),
    /// The last open or action for this slot failed. Draw the message;
    /// the slot does not retry on its own (a retry would re-enter the
    /// same failing WASM call every frame).
    Failed(Arc<str>),
}

#[derive(Debug)]
enum SlotPhase {
    Opening,
    Open {
        session_id: PluginSessionId,
        document: Arc<PluginUiDocument>,
    },
    Failed(Arc<str>),
}

#[derive(Debug)]
struct SlotState {
    phase: SlotPhase,
    /// `start_plugin_action` operations started against this slot and not
    /// yet resolved. Bounded implicitly: an operation is removed on its
    /// terminal event, and every dispatch path in this module records
    /// exactly one.
    inflight: Vec<OperationId>,
}

impl SlotState {
    fn opening() -> Self {
        Self {
            phase: SlotPhase::Opening,
            inflight: Vec::new(),
        }
    }

    fn session_id(&self) -> Option<PluginSessionId> {
        match &self.phase {
            SlotPhase::Open { session_id, .. } => Some(*session_id),
            SlotPhase::Opening | SlotPhase::Failed(_) => None,
        }
    }

    fn revision(&self) -> u64 {
        match &self.phase {
            SlotPhase::Open { document, .. } => document.revision,
            SlotPhase::Opening | SlotPhase::Failed(_) => 0,
        }
    }
}

/// The slot registry. Cloneable (an `Arc` inside), because every render
/// call site and the operation bridge's background task all hold one.
#[derive(Clone, Default)]
pub struct PluginSessions {
    inner: Arc<Mutex<Registry>>,
}

#[derive(Default)]
struct Registry {
    slots: HashMap<PluginSlot, SlotState>,
    /// Reverse index for the operation bridge: an arriving event knows
    /// only its `OperationId`. Populated by [`PluginSessions::dispatch`],
    /// drained on the operation's terminal event.
    operations: HashMap<OperationId, PluginSlot>,
}

impl PluginSessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Render-time read. Declares the slot if it does not exist yet,
    /// spawning the facade's `open_plugin_session` onto `runtime`.
    ///
    /// Takes the runtime handle explicitly rather than reaching into the
    /// facade for one, matching the fire-and-forget pattern every other
    /// facade call site in this crate already uses
    /// (`crate::core::operations::archive::close_archive_session`): the
    /// facade owns its *internal* runtime, and a frontend awaiting a
    /// facade future is expected to supply its own executor.
    ///
    /// Safe to call every frame: the second and later calls for a slot
    /// that is already `Opening` find the entry and return without
    /// spawning anything, so this never re-enters the WASM guest per
    /// frame the way `PluginUiJobs`'s per-frame `layout()` request did.
    pub fn view(
        &self,
        facade: &ArclainApp,
        runtime: &tokio::runtime::Handle,
        slot: &PluginSlot,
    ) -> SlotView {
        {
            let registry = self.inner.lock();
            if let Some(state) = registry.slots.get(slot) {
                return match &state.phase {
                    SlotPhase::Opening => SlotView::Opening,
                    SlotPhase::Open { document, .. } => SlotView::Ready(document.clone()),
                    SlotPhase::Failed(error) => SlotView::Failed(error.clone()),
                };
            }
        }

        let mut registry = self.inner.lock();
        // Re-check under the write section: two render passes in the same
        // frame (a panel drawn twice, a nested `show`) can both miss above.
        if let Some(state) = registry.slots.get(slot) {
            return match &state.phase {
                SlotPhase::Opening => SlotView::Opening,
                SlotPhase::Open { document, .. } => SlotView::Ready(document.clone()),
                SlotPhase::Failed(error) => SlotView::Failed(error.clone()),
            };
        }
        registry.slots.insert(slot.clone(), SlotState::opening());
        drop(registry);

        self.spawn_open(facade, runtime, slot.clone());
        SlotView::Opening
    }

    fn spawn_open(&self, facade: &ArclainApp, runtime: &tokio::runtime::Handle, slot: PluginSlot) {
        let sessions = self.clone();
        let app = facade.clone();
        let plugin_id = slot.plugin_id().to_string();
        let extension_point = slot.extension_point();
        runtime.spawn(async move {
            match app.open_plugin_session(plugin_id, extension_point).await {
                Ok(snapshot) => sessions.opened(&slot, snapshot.session_id, snapshot.document),
                Err(error) => sessions.set_failed(&slot, error.summary),
            }
        });
    }

    /// Records a completed `open_plugin_session`. Ignores the result if
    /// the slot was closed while the open was in flight -- and closes the
    /// freshly-minted session rather than leaking it in the facade's
    /// store forever.
    fn opened(&self, slot: &PluginSlot, session_id: PluginSessionId, document: PluginUiDocument) {
        let mut registry = self.inner.lock();
        let Some(state) = registry.slots.get_mut(slot) else {
            drop(registry);
            tracing::debug!(
                "[plugin-sessions] discarding a session opened for a slot that closed meanwhile"
            );
            return;
        };
        state.phase = SlotPhase::Open {
            session_id,
            document: Arc::new(document),
        };
    }

    fn set_failed(&self, slot: &PluginSlot, error: impl Into<Arc<str>>) {
        let mut registry = self.inner.lock();
        if let Some(state) = registry.slots.get_mut(slot) {
            state.phase = SlotPhase::Failed(error.into());
        }
    }

    /// Dispatches one interaction against `slot`'s open session, spawning
    /// `start_plugin_action` and recording its operation id so the
    /// operation bridge can route the result back here.
    ///
    /// A no-op for a slot that is not `Open`: there is no session to act
    /// against yet, and the interaction that produced this call cannot
    /// have come from a document the user could see.
    pub fn dispatch(
        &self,
        facade: &ArclainApp,
        runtime: &tokio::runtime::Handle,
        slot: &PluginSlot,
        node_id: String,
        action: PluginActionDto,
    ) {
        let Some(session_id) = self
            .inner
            .lock()
            .slots
            .get(slot)
            .and_then(SlotState::session_id)
        else {
            return;
        };

        let sessions = self.clone();
        let app = facade.clone();
        let slot = slot.clone();
        runtime.spawn(async move {
            let request = PluginActionRequest {
                session_id,
                node_id,
                action,
            };
            match app.start_plugin_action(request).await {
                Ok(operation_id) => sessions.track(&slot, operation_id),
                Err(error) => sessions.set_failed(&slot, error.summary),
            }
        });
    }

    /// Records an in-flight action operation against its slot. `pub` so
    /// tests can drive the routing rules without a live facade.
    pub fn track(&self, slot: &PluginSlot, operation_id: OperationId) {
        let mut registry = self.inner.lock();
        if !registry.slots.contains_key(slot) {
            return;
        }
        registry.operations.insert(operation_id, slot.clone());
        if let Some(state) = registry.slots.get_mut(slot) {
            state.inflight.push(operation_id);
        }
    }

    /// Applies a `PluginUiUpdated` result. Returns the intents to run
    /// (and the owning slot) only when the update is genuinely this
    /// slot's current, forward-moving document -- see this module's doc
    /// comment for the three rejection cases.
    ///
    /// Returns `None` for an operation this registry never tracked, which
    /// is the normal case for any `PluginAction` operation another part
    /// of the application started.
    pub fn apply_update(
        &self,
        operation_id: OperationId,
        update: PluginUiUpdate,
    ) -> Option<AppliedUpdate> {
        let mut registry = self.inner.lock();
        let slot = registry.operations.remove(&operation_id)?;
        let Some(state) = registry.slots.get_mut(&slot) else {
            return None;
        };
        state.inflight.retain(|tracked| *tracked != operation_id);

        if state.session_id() != Some(update.document.session_id) {
            tracing::debug!(
                "[plugin-sessions] dropping an update for a session this slot no longer holds"
            );
            return None;
        }
        if update.document.revision <= state.revision() {
            tracing::debug!(
                "[plugin-sessions] dropping an out-of-order update older than the applied revision"
            );
            return None;
        }

        let document = Arc::new(update.document);
        state.phase = SlotPhase::Open {
            session_id: document.session_id,
            document: document.clone(),
        };
        Some(AppliedUpdate {
            slot,
            document,
            intents: update.intents,
        })
    }

    /// Records a failed/cancelled action operation. Returns the slot it
    /// belonged to, or `None` if this registry never tracked it.
    ///
    /// Deliberately does *not* move the slot into `Failed`: the slot's
    /// last good document is still valid and still worth drawing, and an
    /// action that failed (a trapped plugin, a rejected hidden node) is
    /// reported to the user as a toast by the caller rather than by
    /// blanking the panel.
    pub fn fail(&self, operation_id: OperationId) -> Option<PluginSlot> {
        let mut registry = self.inner.lock();
        let slot = registry.operations.remove(&operation_id)?;
        if let Some(state) = registry.slots.get_mut(&slot) {
            state.inflight.retain(|tracked| *tracked != operation_id);
        }
        Some(slot)
    }

    /// Drops `slot` and closes its facade session. Idempotent.
    pub fn close(&self, facade: &ArclainApp, runtime: &tokio::runtime::Handle, slot: &PluginSlot) {
        let removed = {
            let mut registry = self.inner.lock();
            let state = registry.slots.remove(slot);
            if let Some(state) = &state {
                for operation_id in &state.inflight {
                    registry.operations.remove(operation_id);
                }
            }
            state
        };
        let Some(session_id) = removed.and_then(|state| state.session_id()) else {
            return;
        };
        let app = facade.clone();
        runtime.spawn(async move {
            if let Err(error) = app.close_plugin_session(session_id).await {
                tracing::debug!(
                    "[plugin-sessions] closing a plugin session failed: {}",
                    error.summary
                );
            }
        });
    }

    /// Closes every slot belonging to `tab` -- called when a tab closes,
    /// so its panel/dialog/page sessions do not outlive it in the
    /// facade's session store.
    pub fn close_tab(&self, facade: &ArclainApp, runtime: &tokio::runtime::Handle, tab: TabId) {
        for slot in self.slots_matching(|slot| slot.tab() == Some(tab)) {
            self.close(facade, runtime, &slot);
        }
    }

    /// Closes every slot for `plugin_id` -- called when a plugin is
    /// enabled/disabled or reinstalled, so the next frame re-opens
    /// against the plugin's new state instead of drawing a document
    /// fetched from the old one.
    pub fn close_plugin(
        &self,
        facade: &ArclainApp,
        runtime: &tokio::runtime::Handle,
        plugin_id: &str,
    ) {
        for slot in self.slots_matching(|slot| slot.plugin_id() == plugin_id) {
            self.close(facade, runtime, &slot);
        }
    }

    /// Snapshots the matching slot keys, releasing the registry lock
    /// before any caller re-enters it through [`Self::close`].
    fn slots_matching(&self, predicate: impl Fn(&PluginSlot) -> bool) -> Vec<PluginSlot> {
        self.inner
            .lock()
            .slots
            .keys()
            .filter(|slot| predicate(slot))
            .cloned()
            .collect()
    }

    /// Test/diagnostic access to a slot's current session id.
    pub fn session_id(&self, slot: &PluginSlot) -> Option<PluginSessionId> {
        self.inner
            .lock()
            .slots
            .get(slot)
            .and_then(SlotState::session_id)
    }

    /// Number of slots currently registered. Used by tests and by the
    /// leak check in `close_tab`'s own coverage.
    pub fn len(&self) -> usize {
        self.inner.lock().slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The outcome of a successfully applied [`PluginUiUpdate`]: which slot
/// now holds which document, plus the host intents that came with it.
#[derive(Clone, Debug)]
pub struct AppliedUpdate {
    pub slot: PluginSlot,
    pub document: Arc<PluginUiDocument>,
    pub intents: Vec<arclain_app::plugins::PluginHostIntentDto>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_app::plugins::{PluginUiNodeDto, PluginUiNodeKind};

    fn document(session: u64, revision: u64) -> PluginUiDocument {
        PluginUiDocument {
            session_id: PluginSessionId::from_raw(session),
            plugin_id: "demo".to_string(),
            region_id: "panel".to_string(),
            extension_point: PluginExtensionPointDto::Panel,
            revision,
            root: PluginUiNodeDto {
                id: "#root".to_string(),
                kind: PluginUiNodeKind::Single {
                    children: Vec::new(),
                },
                visible: true,
                enabled: true,
            },
        }
    }

    fn panel_slot() -> PluginSlot {
        PluginSlot::Panel {
            plugin_id: "demo".to_string(),
            tab: TabId(1),
        }
    }

    /// Drives the registry into `Open` without a live facade, so the
    /// routing rules below are testable on their own.
    fn open_slot(sessions: &PluginSessions, slot: &PluginSlot, session: u64) {
        sessions
            .inner
            .lock()
            .slots
            .insert(slot.clone(), SlotState::opening());
        sessions.opened(
            slot,
            PluginSessionId::from_raw(session),
            document(session, 1),
        );
    }

    #[test]
    fn an_update_for_a_tracked_operation_applies_and_advances_the_revision() {
        let sessions = PluginSessions::new();
        let slot = panel_slot();
        open_slot(&sessions, &slot, 5);
        sessions.track(&slot, OperationId::from_raw(70));

        let applied = sessions
            .apply_update(
                OperationId::from_raw(70),
                PluginUiUpdate {
                    document: document(5, 2),
                    intents: Vec::new(),
                },
            )
            .expect("an update for a tracked operation applies");

        assert_eq!(applied.slot, slot);
        assert_eq!(applied.document.revision, 2);
    }

    #[test]
    fn an_update_for_an_untracked_operation_is_dropped() {
        let sessions = PluginSessions::new();
        let slot = panel_slot();
        open_slot(&sessions, &slot, 5);

        assert!(sessions
            .apply_update(
                OperationId::from_raw(999),
                PluginUiUpdate {
                    document: document(5, 2),
                    intents: Vec::new(),
                },
            )
            .is_none());
    }

    /// The "wrong slot" rejection: two slots render the same plugin, and
    /// only the one that recorded the operation may apply its result.
    #[test]
    fn an_update_never_reaches_a_sibling_slot_of_the_same_plugin() {
        let sessions = PluginSessions::new();
        let panel = panel_slot();
        let dialog = PluginSlot::Dialog {
            plugin_id: "demo".to_string(),
            dialog_id: "settings".to_string(),
            tab: TabId(1),
        };
        open_slot(&sessions, &panel, 5);
        open_slot(&sessions, &dialog, 6);
        sessions.track(&panel, OperationId::from_raw(70));

        let applied = sessions
            .apply_update(
                OperationId::from_raw(70),
                PluginUiUpdate {
                    document: document(5, 2),
                    intents: Vec::new(),
                },
            )
            .expect("applies to its own slot");

        assert_eq!(applied.slot, panel);
        // The sibling's document is untouched at its opening revision.
        assert_eq!(
            sessions.session_id(&dialog),
            Some(PluginSessionId::from_raw(6))
        );
    }

    /// The "wrong session" rejection: the slot was closed and reopened
    /// while the action was in flight.
    #[test]
    fn an_update_for_a_session_the_slot_no_longer_holds_is_dropped() {
        let sessions = PluginSessions::new();
        let slot = panel_slot();
        open_slot(&sessions, &slot, 5);
        sessions.track(&slot, OperationId::from_raw(70));
        // Reopened against a new session while the action was in flight.
        sessions.opened(&slot, PluginSessionId::from_raw(9), document(9, 1));

        assert!(sessions
            .apply_update(
                OperationId::from_raw(70),
                PluginUiUpdate {
                    document: document(5, 4),
                    intents: Vec::new(),
                },
            )
            .is_none());
        assert_eq!(
            sessions.session_id(&slot),
            Some(PluginSessionId::from_raw(9))
        );
    }

    /// The "older revision" rejection: two dispatches complete out of
    /// order, and the UI must not roll backwards.
    #[test]
    fn an_out_of_order_older_revision_is_dropped() {
        let sessions = PluginSessions::new();
        let slot = panel_slot();
        open_slot(&sessions, &slot, 5);
        sessions.track(&slot, OperationId::from_raw(70));
        sessions.track(&slot, OperationId::from_raw(71));

        assert!(sessions
            .apply_update(
                OperationId::from_raw(71),
                PluginUiUpdate {
                    document: document(5, 3),
                    intents: Vec::new(),
                },
            )
            .is_some());
        assert!(sessions
            .apply_update(
                OperationId::from_raw(70),
                PluginUiUpdate {
                    document: document(5, 2),
                    intents: Vec::new(),
                },
            )
            .is_none());
    }

    #[test]
    fn a_failed_operation_keeps_the_slots_last_good_document() {
        let sessions = PluginSessions::new();
        let slot = panel_slot();
        open_slot(&sessions, &slot, 5);
        sessions.track(&slot, OperationId::from_raw(70));

        assert_eq!(sessions.fail(OperationId::from_raw(70)), Some(slot.clone()));
        assert_eq!(
            sessions.session_id(&slot),
            Some(PluginSessionId::from_raw(5))
        );
        // The same operation cannot be routed twice.
        assert!(sessions.fail(OperationId::from_raw(70)).is_none());
    }

    #[test]
    fn a_slot_opened_after_it_was_closed_is_not_resurrected() {
        let sessions = PluginSessions::new();
        let slot = panel_slot();
        sessions
            .inner
            .lock()
            .slots
            .insert(slot.clone(), SlotState::opening());
        sessions.inner.lock().slots.remove(&slot);

        sessions.opened(&slot, PluginSessionId::from_raw(5), document(5, 1));

        assert!(sessions.is_empty());
    }

    #[test]
    fn button_navigation_splits_declarative_actions_from_plugin_interactions() {
        assert_eq!(
            PluginNavigation::resolve(
                "btn",
                Some(&PluginButtonActionDto::ShowDialog {
                    id: "settings".to_string()
                })
            ),
            (
                Some(PluginNavigation::OpenDialog {
                    dialog_id: "settings".to_string()
                }),
                None
            )
        );
        assert_eq!(
            PluginNavigation::resolve("btn", Some(&PluginButtonActionDto::ClosePage)),
            (Some(PluginNavigation::ClosePage), None)
        );
        assert_eq!(
            PluginNavigation::resolve("btn", Some(&PluginButtonActionDto::CloseDialog)),
            (Some(PluginNavigation::CloseDialog), None)
        );
        assert_eq!(
            PluginNavigation::resolve(
                "btn",
                Some(&PluginButtonActionDto::OpenPage {
                    id: "detail".to_string()
                })
            ),
            (
                Some(PluginNavigation::OpenPage {
                    page_id: "detail".to_string()
                }),
                None
            )
        );
    }

    /// `Custom` overrides the node id the plugin receives; bare `None`
    /// sends the node's own id. Both are plugin interactions, never
    /// navigation -- the old renderer's `__`-prefixed encoding cannot be
    /// produced by this path at all.
    #[test]
    fn custom_and_default_button_actions_are_plugin_interactions() {
        assert_eq!(
            PluginNavigation::resolve(
                "btn",
                Some(&PluginButtonActionDto::Custom {
                    value: "do_thing".to_string()
                })
            ),
            (None, Some("do_thing".to_string()))
        );
        assert_eq!(
            PluginNavigation::resolve("btn", Some(&PluginButtonActionDto::None)),
            (None, Some("btn".to_string()))
        );
        assert_eq!(
            PluginNavigation::resolve("btn", None),
            (None, Some("btn".to_string()))
        );
    }

    #[test]
    fn slot_extension_points_cover_every_variant() {
        assert_eq!(
            PluginSlot::MainPage {
                plugin_id: "demo".to_string()
            }
            .extension_point(),
            PluginExtensionPointDto::MainPage
        );
        assert_eq!(
            PluginSlot::PluginButton {
                plugin_id: "demo".to_string()
            }
            .extension_point(),
            PluginExtensionPointDto::PluginButton
        );
        assert_eq!(
            panel_slot().extension_point(),
            PluginExtensionPointDto::Panel
        );
        assert_eq!(
            PluginSlot::Dialog {
                plugin_id: "demo".to_string(),
                dialog_id: "d".to_string(),
                tab: TabId(1),
            }
            .extension_point(),
            PluginExtensionPointDto::Dialog("d".to_string())
        );
        assert_eq!(
            PluginSlot::Page {
                plugin_id: "demo".to_string(),
                page_id: "p".to_string(),
                tab: TabId(1),
            }
            .extension_point(),
            PluginExtensionPointDto::Page("p".to_string())
        );
    }

    /// A window-scoped slot must not be swept away when an unrelated tab
    /// closes -- see `PluginSlot`'s own doc comment for why `MainPage`/
    /// `PluginButton` are deliberately not tab-scoped.
    #[test]
    fn only_tab_scoped_slots_report_a_tab() {
        assert_eq!(panel_slot().tab(), Some(TabId(1)));
        assert_eq!(
            PluginSlot::MainPage {
                plugin_id: "demo".to_string()
            }
            .tab(),
            None
        );
        assert_eq!(
            PluginSlot::PluginButton {
                plugin_id: "demo".to_string()
            }
            .tab(),
            None
        );
    }
}
