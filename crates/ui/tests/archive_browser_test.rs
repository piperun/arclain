use arclain_ui::features::archive_browser::{Action, BrowserController};
use arclain_ui::features::archive_operations::ArchiveOperationsState;
use arclain_ui::shared::models::file_entry::FileEntry;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

mod common;
use common::TestContext;

trait HandleAction {
    fn handle_action(&mut self, action: Action);
}

impl HandleAction for TestContext {
    fn handle_action(&mut self, action: Action) {
        let controller = BrowserController::new();
        // Since we removed ops_state, we need to mock it or update the test to not rely on it being passed explicitly
        // However, handle_action now takes fewer arguments.
        // We'll create a temporary ops_state for the test context if needed by the controller (it's not, used signals now mostly)
        // Wait, controller.handle_action takes:
        // (action, shared, ops_state, org_feature, navigator, ctx)
        // checking the signature from previous turn...
        // It takes: action, shared, archive_ops_state, organization_feature, page_navigator, ctx

        let mut ops_state = ArchiveOperationsState::default();

        controller.handle_action(
            action,
            &self.shared,
            &mut ops_state,
            &mut self.org_feature,
            &mut self.navigator,
            &self.egui_ctx,
        );
    }
}

// --- Logic Tests ---

#[test]
fn test_navigate_to_folder_action() {
    let mut ctx = TestContext::new();
    let signals = ctx.shared.app_state.lock().signals.clone();

    signals
        .tabs
        .get()
        .active()
        .archive_path
        .set(Some(PathBuf::from("test.zip")));

    let target = "subfolder".to_string();
    ctx.handle_action(Action::NavigateToFolder(target.clone()));

    assert_eq!(
        signals.tabs.get().active().navigation.get().current_path,
        target
    );
}

#[test]
fn test_navigate_to_path_action() {
    let mut ctx = TestContext::new();
    let signals = ctx.shared.app_state.lock().signals.clone();
    signals
        .tabs
        .get()
        .active()
        .archive_path
        .set(Some(PathBuf::from("test.zip")));

    let target = "direct/path/folder".to_string();
    ctx.handle_action(Action::NavigateToPath(target.clone()));

    assert_eq!(
        signals.tabs.get().active().navigation.get().current_path,
        target
    );
}

#[test]
fn test_show_properties_action() {
    let mut ctx = TestContext::new();
    let signals = ctx.shared.app_state.lock().signals.clone();

    // Setup entries via signal
    let tab = signals.tabs.get().active().clone();
    tab.browser_entries.update(|snapshot| {
        snapshot.replace(vec![FileEntry {
            name: "test.txt".to_string(),
            path: "test.txt".to_string(),
            size: "100".to_string(),
            compressed: "50".to_string(),
            ratio: "50%".to_string(),
            modified: "2024-01-01".to_string(),
            crc32: "00000000".to_string(),
            encrypted: false,
            is_folder: false,
        }])
    });

    ctx.handle_action(Action::ShowProperties("test.txt".to_string()));

    let view_state = tab.browser_view_state.get();
    assert!(view_state.toolbar_state.show_properties_panel);
    // Selection lives in a path-keyed HashSet now; assert the path is
    // in there rather than reading `entry.selected` (which was removed
    // from FileEntry to dodge the worker/renderer data race).
    assert!(view_state.selection.contains("test.txt"));
}

#[test]
fn test_copy_path_action() {
    let mut ctx = TestContext::new();
    let signals = ctx.shared.app_state.lock().signals.clone();

    signals
        .tabs
        .get()
        .active()
        .navigation
        .get()
        .set_current_path("root/folder");

    let filename = "file.txt".to_string();
    ctx.handle_action(Action::CopyPath(filename.clone()));

    signals
        .tabs
        .get()
        .active()
        .navigation
        .get()
        .set_current_path("");
    ctx.handle_action(Action::CopyPath(filename));
}

#[test]
fn test_open_file_action() {
    let mut ctx = TestContext::new();
    let signals = ctx.shared.app_state.lock().signals.clone();
    let filename = "document.pdf".to_string();

    ctx.handle_action(Action::OpenFile(filename.clone()));

    // pending_open_file is per-tab now (2026-05-19 audit)
    assert_eq!(
        signals.tabs.get().active().pending_open_file.get(),
        Some(filename)
    );
}

#[test]
fn test_edit_file_action() {
    let mut ctx = TestContext::new();
    let signals = ctx.shared.app_state.lock().signals.clone();
    let filename = "config.json".to_string();
    ctx.handle_action(Action::EditFile(filename.clone()));

    // file_edit_dialog is per-tab now (2026-05-19 audit)
    let dialog = signals.tabs.get().active().file_edit_dialog.get();
    assert!(dialog.show);
    assert_eq!(dialog.full_path_in_archive, filename);
    assert_eq!(dialog.name_input, filename);
}

#[test]
fn test_metadata_action() {
    let mut ctx = TestContext::new();
    let signals = ctx.shared.app_state.lock().signals.clone();

    // Note: circle -> handled by GameMetadata::from_json mapping if creator is missing
    // But direct JSON deserialization might not run from_json logic if we just parse raw JSON in the action handler.
    // Let's check Action::Metadata handler in actions.rs.
    // It calls serde_json::from_str::<GameMetadata>. This uses derived Deserialize, NOT from_json.
    // So "circle" in JSON won't map to "creator" unless we use custom deserialization or the handler uses from_json.
    // Checked actions.rs: it calls `serde_json::from_str`.
    // Valid GameMetadata JSON requires fields matching struct or being optional.

    let json = r#"{"product_id": "RJ1", "source": "dlsite", "title": "Test Game", "tags": [], "metadata_json": "{}", "screenshots": []}"#.to_string();
    ctx.handle_action(Action::Metadata(json));

    let metadata = signals.tabs.get().active().game_metadata.get();
    assert!(metadata.is_some());
    let meta = metadata.unwrap();
    assert_eq!(meta.title, "Test Game");
    assert_eq!(meta.product_id, "RJ1");
}

#[test]
fn test_organize_action() {
    let mut ctx = TestContext::new();
    let signals = ctx.shared.app_state.lock().signals.clone();

    signals
        .tabs
        .get()
        .active()
        .archive_path
        .set(Some(PathBuf::from("test.zip")));

    ctx.handle_action(Action::Organize);

    // Verify navigation
    if let arclain_ui::core::AppPage::OrganizeArchive(name) = &ctx.navigator.current_page {
        assert_eq!(name, "test.zip");
    } else {
        panic!(
            "Expected OrganizeArchive page, got {:?}",
            ctx.navigator.current_page
        );
    }

    // Verify feature state
    assert!(ctx.org_feature.organizer_page.is_some());
}

// --- UI Integration Tests ---

#[test]
fn test_ui_render_sanity() {
    use arclain_ui::features::archive_browser::ArchiveBrowser;
    use egui_kittest::Harness;

    let ctx = TestContext::new();
    let shared = ctx.shared.clone();
    let mut browser = ArchiveBrowser::new(&shared);

    // Setup state
    shared
        .signals()
        .tabs
        .get()
        .active()
        .archive_path
        .set(Some(PathBuf::from("test.zip")));

    let tab = shared.signals().tabs.get().active().clone();
    tab.browser_entries.update(|snapshot| {
        snapshot.replace(vec![FileEntry {
            name: "test_ui_file.txt".to_string(),
            path: "test_ui_file.txt".to_string(),
            size: "100".to_string(),
            compressed: "50".to_string(),
            ratio: "50%".to_string(),
            modified: "2024-01-01".to_string(),
            crc32: "00000000".to_string(),
            encrypted: false,
            is_folder: false,
        }])
    });

    // egui_kittest harness
    let mut harness = Harness::new(move |ctx| {
        let _ = browser.render(ctx, &shared);
    });

    harness.run();

    // In a real scenario with AccessKit support enabled in egui_kittest (requires feature flags or config),
    // we could do: harness.get_by_label("test_ui_file.txt").exists();
    // For now, this confirms the render loop completes without panicking on missing resources.
}

#[test]
fn idle_render_reuses_entry_allocation_without_publishing_state() {
    use arclain_ui::features::archive_browser::ArchiveBrowser;
    use egui_kittest::Harness;

    let ctx = TestContext::new();
    let shared = ctx.shared.clone();
    let tab = shared.signals().tabs.get().active().clone();
    tab.archive_path.set(Some(PathBuf::from("large.zip")));

    let entries = (0..10_000)
        .rev()
        .map(|index| FileEntry {
            name: format!("entry-{index:05}.txt"),
            path: format!("entry-{index:05}.txt"),
            size: "0 B".to_string(),
            compressed: "0 B".to_string(),
            ratio: "0%".to_string(),
            modified: String::new(),
            crc32: String::new(),
            encrypted: false,
            is_folder: false,
        })
        .collect();
    tab.browser_entries
        .update(|snapshot| snapshot.replace(entries));
    let before = tab.browser_entries.get();

    let entry_notifications = Arc::new(AtomicUsize::new(0));
    let view_notifications = Arc::new(AtomicUsize::new(0));
    let entry_notifications_for_listener = entry_notifications.clone();
    tab.browser_entries.subscribe(move || {
        entry_notifications_for_listener.fetch_add(1, Ordering::SeqCst);
    });
    let view_notifications_for_listener = view_notifications.clone();
    tab.browser_view_state.subscribe(move || {
        view_notifications_for_listener.fetch_add(1, Ordering::SeqCst);
    });

    let mut browser = ArchiveBrowser::new(&shared);
    let mut harness = Harness::new(move |ctx| {
        let _ = browser.render(ctx, &shared);
    });
    harness.run_steps(2);

    let after = tab.browser_entries.get();
    assert!(Arc::ptr_eq(&before.entries, &after.entries));
    assert_eq!(entry_notifications.load(Ordering::SeqCst), 0);
    assert_eq!(view_notifications.load(Ordering::SeqCst), 0);
}
