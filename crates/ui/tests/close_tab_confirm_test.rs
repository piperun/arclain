//! egui_kittest interaction tests for the close-tab confirmation modal.
//!
//! The modal shows when the user tries to close a tab that has in-flight
//! operations. It exposes two button choices (Close / Cancel) so the
//! user makes an explicit decision about cancelling work in progress.
//!
//! These tests verify:
//! - both buttons render as queryable widgets,
//! - clicking Cancel emits `Cancelled` and hides the modal,
//! - clicking Close emits `Confirmed(id)` and hides the modal.
//!
//! Catches regressions where the labels drift (e.g. someone renames
//! "Close" to "OK") since the dialog handler keys off the result enum,
//! not the labels — but a user-facing label drift is still a UX bug
//! worth surfacing in CI.

use arclain_ui::core::tabs::TabId;
use arclain_ui::shared::dialogs::{
    render_close_tab_confirm, CloseTabConfirmResult, CloseTabConfirmState,
};
use arclain_ui::shared::theme::AppTheme;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;

/// Bundles the modal state with the last emitted `Result` so the click
/// handler can observe it across frames without juggling a separate
/// RefCell. The harness state must be `'static`, so `theme` lives here
/// too instead of being captured by the render closure.
struct ModalHarness {
    state: CloseTabConfirmState,
    last_result: CloseTabConfirmResult,
    theme: AppTheme,
}

fn populated_harness() -> ModalHarness {
    ModalHarness {
        state: CloseTabConfirmState {
            show: true,
            tab_id: Some(TabId(42)),
            tab_title: "busy.zip".to_string(),
            in_flight_count: 2,
        },
        last_result: CloseTabConfirmResult::None,
        theme: AppTheme::default(),
    }
}

#[test]
fn modal_renders_close_and_cancel_buttons() {
    let mut harness = Harness::new_ui_state(
        |ui, h: &mut ModalHarness| {
            let r = render_close_tab_confirm(ui.ctx(), &h.theme, &mut h.state);
            // Capture the FIRST non-None result. After the click frame
            // hides the modal, subsequent frames return None — without
            // this latch the click outcome would be overwritten before
            // the test could inspect it.
            if !matches!(r, CloseTabConfirmResult::None) {
                h.last_result = r;
            }
        },
        populated_harness(),
    );
    harness.run();
    assert!(
        harness.query_all_by_label("Close").next().is_some(),
        "Close button should be a queryable widget"
    );
    assert!(
        harness.query_all_by_label("Cancel").next().is_some(),
        "Cancel button should be a queryable widget"
    );
}

#[test]
fn cancel_button_hides_modal_and_returns_cancelled() {
    let mut harness = Harness::new_ui_state(
        |ui, h: &mut ModalHarness| {
            let r = render_close_tab_confirm(ui.ctx(), &h.theme, &mut h.state);
            // Capture the FIRST non-None result. After the click frame
            // hides the modal, subsequent frames return None — without
            // this latch the click outcome would be overwritten before
            // the test could inspect it.
            if !matches!(r, CloseTabConfirmResult::None) {
                h.last_result = r;
            }
        },
        populated_harness(),
    );
    harness.run();
    harness.get_by_label("Cancel").click();
    // egui's button responds on PointerReleased; run two more frames
    // so the press → release → result-emit chain completes.
    harness.run();
    harness.run();

    assert_eq!(harness.state().last_result, CloseTabConfirmResult::Cancelled);
    assert!(
        !harness.state().state.show,
        "Cancel must hide the modal so the next frame doesn't re-show it"
    );
}

#[test]
fn close_button_returns_confirmed_with_tab_id_and_hides_modal() {
    let mut harness = Harness::new_ui_state(
        |ui, h: &mut ModalHarness| {
            let r = render_close_tab_confirm(ui.ctx(), &h.theme, &mut h.state);
            // Capture the FIRST non-None result. After the click frame
            // hides the modal, subsequent frames return None — without
            // this latch the click outcome would be overwritten before
            // the test could inspect it.
            if !matches!(r, CloseTabConfirmResult::None) {
                h.last_result = r;
            }
        },
        populated_harness(),
    );
    harness.run();
    harness.get_by_label("Close").click();
    harness.run();
    harness.run();

    assert_eq!(
        harness.state().last_result,
        CloseTabConfirmResult::Confirmed(TabId(42))
    );
    assert!(!harness.state().state.show);
}

#[test]
fn modal_is_inert_when_show_is_false() {
    // When state.show is false the function should bail before any
    // widgets are added — none of the buttons should be queryable.
    let mut initial = populated_harness();
    initial.state.show = false;
    let mut harness = Harness::new_ui_state(
        |ui, h: &mut ModalHarness| {
            let r = render_close_tab_confirm(ui.ctx(), &h.theme, &mut h.state);
            // Capture the FIRST non-None result. After the click frame
            // hides the modal, subsequent frames return None — without
            // this latch the click outcome would be overwritten before
            // the test could inspect it.
            if !matches!(r, CloseTabConfirmResult::None) {
                h.last_result = r;
            }
        },
        initial,
    );
    harness.run();
    assert!(harness.query_all_by_label("Close").next().is_none());
    assert!(harness.query_all_by_label("Cancel").next().is_none());
}
