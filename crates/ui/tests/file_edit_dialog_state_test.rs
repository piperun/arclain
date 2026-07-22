use arclain_ui::features::file_editing::domain::types::FileEditLoadState;
use arclain_ui::features::file_editing::{render_file_edit_dialog, FileEditDialog, FileEditResult};
use arclain_ui::shared::theme::AppTheme;
use eframe::egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_kittest::Harness;

#[derive(Debug, Default, PartialEq, Eq)]
enum EmittedResult {
    #[default]
    None,
    Save {
        new_name: String,
        content: String,
    },
    Cancel,
}

struct EditDialogHarness {
    dialog: FileEditDialog,
    emitted: EmittedResult,
    theme: AppTheme,
}

fn dialog(load_state: FileEditLoadState) -> FileEditDialog {
    FileEditDialog {
        show: true,
        full_path_in_archive: "notes.txt".to_string(),
        name_input: "notes.txt".to_string(),
        content: "loaded content".to_string(),
        original_content: "loaded content".to_string(),
        error: String::new(),
        load_state,
    }
}

fn render_dialog(ui: &mut eframe::egui::Ui, harness: &mut EditDialogHarness) {
    if let Some(result) = render_file_edit_dialog(ui.ctx(), &harness.theme, &mut harness.dialog) {
        harness.emitted = match result {
            FileEditResult::Save { new_name, content } => EmittedResult::Save { new_name, content },
            FileEditResult::Cancel => EmittedResult::Cancel,
        };
    }
}

fn harness(load_state: FileEditLoadState) -> Harness<'static, EditDialogHarness> {
    Harness::new_ui_state(
        render_dialog,
        EditDialogHarness {
            dialog: dialog(load_state),
            emitted: EmittedResult::None,
            theme: AppTheme::default(),
        },
    )
}

fn assert_no_editors_and_disabled_save(harness: &Harness<'_, EditDialogHarness>) {
    assert!(
        harness.query_by_role(Role::TextInput).is_none(),
        "file name must not be editable until the read succeeds"
    );
    assert!(
        harness.query_by_role(Role::MultilineTextInput).is_none(),
        "file content must not be editable until the read succeeds"
    );
    assert!(
        harness.get_by_label("Save").accesskit_node().is_disabled(),
        "Save must be disabled until the read succeeds"
    );
    assert!(
        !harness
            .get_by_label("Cancel")
            .accesskit_node()
            .is_disabled(),
        "Cancel must remain usable"
    );
}

#[test]
fn loading_dialog_has_no_editor_and_only_cancel_is_usable() {
    let mut harness = harness(FileEditLoadState::Loading { request_id: 7 });
    // The loading spinner intentionally repaints continuously, so advance a
    // bounded number of frames instead of waiting for the UI to become idle.
    harness.run_steps(1);

    assert!(harness.query_by_label("Loading file content…").is_some());
    assert_no_editors_and_disabled_save(&harness);

    harness.get_by_label("Cancel").click();
    harness.run_steps(2);
    assert_eq!(harness.state().emitted, EmittedResult::Cancel);
}

#[test]
fn failed_dialog_shows_error_and_only_cancel_is_usable() {
    let mut failed = dialog(FileEditLoadState::Failed("archive read failed".to_string()));
    failed.error = "archive read failed".to_string();
    let mut harness = Harness::new_ui_state(
        render_dialog,
        EditDialogHarness {
            dialog: failed,
            emitted: EmittedResult::None,
            theme: AppTheme::default(),
        },
    );
    harness.run();

    assert!(harness.query_by_label("Unable to load file").is_some());
    assert!(harness.query_by_label("archive read failed").is_some());
    assert_no_editors_and_disabled_save(&harness);

    harness.get_by_label("Cancel").click();
    harness.run();
    harness.run();
    assert_eq!(harness.state().emitted, EmittedResult::Cancel);
}

#[test]
fn ready_dialog_has_enabled_editors_and_save_emits_loaded_content() {
    let mut harness = harness(FileEditLoadState::Ready);
    harness.run();

    let name_editor = harness.get_by_role(Role::TextInput);
    let content_editor = harness.get_by_role(Role::MultilineTextInput);
    assert!(!name_editor.accesskit_node().is_disabled());
    assert!(!content_editor.accesskit_node().is_disabled());
    assert!(!harness.get_by_label("Save").accesskit_node().is_disabled());

    harness.get_by_label("Save").click();
    harness.run();
    harness.run();
    assert_eq!(
        harness.state().emitted,
        EmittedResult::Save {
            new_name: "notes.txt".to_string(),
            content: "loaded content".to_string(),
        }
    );
}
