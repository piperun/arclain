use arclain_ui::core::navigation::PageNavigator;
use arclain_ui::core::services::Services;
use arclain_ui::core::state::AppState;
use arclain_ui::features::archive_browser::{Action, BrowserController};
use arclain_ui::features::archive_operations::ArchiveOperationsState;
use arclain_ui::features::organization::OrganizationFeature;
use arclain_ui::shared::models::file_entry::FileEntry;
use arclain_ui::shared::theme::AppTheme;
use arclain_ui::shared::SharedState;
use arclain_widgets::Toaster;

use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::backends::BackendSelector;
use arclain_core::UserConfig;
use eframe::egui;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::runtime::Runtime;

// Helper to create a minimal SharedState for testing
fn create_test_shared_state() -> SharedState {
    let runtime = Runtime::new().unwrap();
    let services = Arc::new(Services::new(runtime));

    // Create minimal AppState
    let app_state = AppState {
        user_config: UserConfig::default(),
        pass_rules: vec![],
        backend_selector: BackendSelector::new_native(),
        fallback_backend: SevenZipCli::detect(None).expect("7z executable not found for tests"),
        last_entries: vec![],
        encrypted_crc_policy: "on_open".to_string(),
        db_paths: None,
        dbs: None,
        plugin_event_sender: None,
        pending_plugin_event: None,
        signals: arclain_ui::core::signals::AppSignals::new(),
    };

    let signals = app_state.signals.clone();

    SharedState {
        app_state: Arc::new(Mutex::new(app_state)),
        services,
        theme: AppTheme::new(false),
        toaster: Arc::new(Mutex::new(Toaster::new())),
        refresh_requests: Arc::new(Mutex::new(Vec::new())),
        pending_plugin_actions: Arc::new(Mutex::new(Vec::new())),
        signals,
    }
}

pub struct TestContext {
    pub shared: SharedState,
    pub navigator: PageNavigator,
    pub org_feature: OrganizationFeature,
    pub egui_ctx: egui::Context,
}

impl TestContext {
    fn new() -> Self {
        let shared = create_test_shared_state();
        Self {
            org_feature: OrganizationFeature::new(&shared),
            shared,
            navigator: PageNavigator::new(),
            egui_ctx: egui::Context::default(),
        }
    }

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

    signals.tabs.get().active().archive_path.set(Some(PathBuf::from("test.zip")));

    let target = "subfolder".to_string();
    ctx.handle_action(Action::NavigateToFolder(target.clone()));

    assert_eq!(signals.tabs.get().active().navigation.get().current_path, target);
}

#[test]
fn test_navigate_to_path_action() {
    let mut ctx = TestContext::new();
    let signals = ctx.shared.app_state.lock().signals.clone();
    signals.tabs.get().active().archive_path.set(Some(PathBuf::from("test.zip")));

    let target = "direct/path/folder".to_string();
    ctx.handle_action(Action::NavigateToPath(target.clone()));

    assert_eq!(signals.tabs.get().active().navigation.get().current_path, target);
}

#[test]
fn test_show_properties_action() {
    let mut ctx = TestContext::new();
    let signals = ctx.shared.app_state.lock().signals.clone();

    // Setup entries via signal
    let tab = signals.tabs.get().active().clone();
    let mut view_state = tab.browser_view_state.get();
    view_state.view_entries.push(FileEntry {
        name: "test.txt".to_string(),
        path: "test.txt".to_string(),
        size: "100".to_string(),
        compressed: "50".to_string(),
        ratio: "50%".to_string(),
        modified: "2024-01-01".to_string(),
        crc32: "00000000".to_string(),
        encrypted: false,
        is_folder: false,
    });
    tab.browser_view_state.set(view_state);

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

    signals.tabs.get().active().navigation.get().set_current_path("root/folder");

    let filename = "file.txt".to_string();
    ctx.handle_action(Action::CopyPath(filename.clone()));

    signals.tabs.get().active().navigation.get().set_current_path("");
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

    signals.tabs.get().active().archive_path.set(Some(PathBuf::from("test.zip")));

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
    let mut view_state = tab.browser_view_state.get();
    view_state.view_entries.push(FileEntry {
        name: "test_ui_file.txt".to_string(),
        path: "test_ui_file.txt".to_string(),
        size: "100".to_string(),
        compressed: "50".to_string(),
        ratio: "50%".to_string(),
        modified: "2024-01-01".to_string(),
        crc32: "00000000".to_string(),
        encrypted: false,
        is_folder: false,
    });
    tab.browser_view_state.set(view_state);

    // egui_kittest harness
    let mut harness = Harness::new(move |ctx| {
        let _ = browser.render(ctx, &shared);
    });

    harness.run();

    // In a real scenario with AccessKit support enabled in egui_kittest (requires feature flags or config),
    // we could do: harness.get_by_label("test_ui_file.txt").exists();
    // For now, this confirms the render loop completes without panicking on missing resources.
}
