use arclain_ui::features::archive_browser::application::file_ops_service::{
    TextReadFuture, TextReadIo,
};
use arclain_ui::features::archive_browser::application::FileOpsService;
use arclain_ui::features::archive_browser::{Action, BrowserController};
use arclain_ui::features::archive_operations::ArchiveOperationsState;
use arclain_ui::features::file_editing::domain::types::{FileEditDialog, FileEditLoadState};
use arclain_ui::shared::models::file_entry::FileEntry;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::thread;
use std::time::{Duration, Instant};

mod common;
use common::TestContext;

/// Seeds a tab's whole-archive inventory with fabricated facade rows --
/// what the bridge's relist would adopt from `list_all_entries` in
/// production. Test-constructed `EntryId`s are fine here: these tests
/// never hand them back to a facade, they only feed the render-side
/// projections.
fn seed_inventory(tab: &arclain_ui::core::tabs::TabState, paths: &[String]) {
    let rows: Vec<(String, arclain_app::archive::EntryKind)> = paths
        .iter()
        .map(|path| (path.clone(), arclain_app::archive::EntryKind::File))
        .collect();
    seed_inventory_rows(tab, 1, &rows);
}

/// [`seed_inventory`] with an explicit revision and per-row kind, so a
/// test can seat a *refreshed* inventory (a higher revision) and can
/// include the directory rows the session's index synthesizes.
fn seed_inventory_rows(
    tab: &arclain_ui::core::tabs::TabState,
    revision: u64,
    rows: &[(String, arclain_app::archive::EntryKind)],
) {
    use arclain_app::archive::{ArchiveEntryDto, ArchiveInventory, ArchivePath};
    use arclain_ui::core::tabs::{AdoptedInventory, TabInventory};

    let session_id = arclain_app::ids::ArchiveSessionId::from_raw(1);
    let inventory = ArchiveInventory {
        session_id,
        revision,
        entries: rows
            .iter()
            .enumerate()
            .map(|(index, (path, kind))| ArchiveEntryDto {
                id: arclain_app::ids::EntryId::from_raw(index as u64 + 1),
                path: ArchivePath::parse(path.clone()).unwrap(),
                name: path.rsplit('/').next().unwrap_or(path).to_string(),
                kind: kind.clone(),
                compressed_size: Some(0),
                uncompressed_size: 0,
                // 2024-01-15 10:30:00 UTC. Only the DTO carries a real
                // timestamp; a row showing it can only have come from
                // one.
                modified_at_unix_ms: Some(1_705_314_600_000),
                encrypted: false,
                crc32: None,
            })
            .collect(),
    };
    let prepared = AdoptedInventory::prepare(inventory);
    tab.inventory.update(|held| {
        if held.session() != Some(session_id) {
            *held = TabInventory::for_session(Some(session_id));
        }
        held.adopt(prepared.clone());
    });
    settle_listing(tab, session_id);
}

/// Records that the tab's listing answered for the directory it is
/// browsing, bound to `session_id`.
///
/// Seating an inventory *is* a listing having answered: in production the
/// same fetch brackets the adopt with `begin_loading`/`succeed`, and
/// nothing publishes a browser row before that pair closes. A fixture that
/// seats rows while leaving the listing having asked for nothing describes
/// a tab production cannot produce -- and the browser rightly draws that
/// as an archive whose contents it does not have yet, since a tab merely
/// *pointed* at an archive looks exactly the same.
fn settle_listing(
    tab: &arclain_ui::core::tabs::TabState,
    session_id: arclain_app::ids::ArchiveSessionId,
) {
    use arclain_ui::core::tabs::TabListing;

    tab.listing.update(|listing| {
        if listing.session() != Some(session_id) {
            *listing = TabListing::for_session(Some(session_id));
        }
        let directory = listing.directory().clone();
        let generation = listing.begin_loading();
        assert!(listing.succeed(generation, session_id, &directory));
    });
}

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

/// A text read whose completion the test controls: it announces the path
/// it was asked for, then parks until the test opens its gate.
///
/// The park runs on the blocking pool rather than inline in the future,
/// because the runtime backing these tests has two workers and one test
/// keeps two reads outstanding at once -- blocking both workers would
/// stall the very completion tasks the assertions wait for.
struct BlockingReadIo {
    started: Sender<String>,
    gate: Arc<StdMutex<Receiver<()>>>,
    content: String,
}

impl TextReadIo for BlockingReadIo {
    fn read_text(&self, path: String) -> TextReadFuture<'_> {
        self.started.send(path).unwrap();
        let gate = self.gate.clone();
        let content = self.content.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || gate.lock().unwrap().recv().unwrap())
                .await
                .unwrap();
            Ok(content)
        })
    }
}

fn wait_until(message: &str, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !predicate() {
        assert!(Instant::now() < deadline, "{message}");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn archive_test_context_bounds_runtime_workers_for_parallel_tests() {
    let ctx = TestContext::new();

    assert_eq!(
        ctx.shared.services.tokio_runtime.metrics().num_workers(),
        2,
        "each parallel archive test must not create one Tokio worker per CPU"
    );
}

// The two delete-specific fakes/tests that used to live here
// (`BlockingDeleteIo`/`OrderedDeleteIo`, exercising `delete_files_with_io`'s
// serialization against `origin.archive_edit_lock`) tested a mechanism
// that no longer exists: delete now goes through
// `arclain_app::ArclainApp::start_archive_mutation`, not an injectable
// `ArchiveFileIo::delete_and_list`. Its equivalent guarantee --
// serializing concurrent mutations on one archive -- is now
// `ArchiveSession::mutation_lock`'s job, covered by
// `crates/app/tests/archive_mutation.rs`'s own
// `cancelling_a_mutation_queued_behind_another_one_on_the_same_session_never_reaches_the_backend`
// test. `crates/ui/tests/archive_mutation_ui_test.rs` covers this crate's
// own add/delete/save wiring end to end against a real bootstrapped
// facade.

#[test]
fn archive_file_jobs_text_read_returns_before_io_and_updates_origin_tab() {
    let ctx = TestContext::new();
    let shared = ctx.shared.clone();
    let origin = shared.signals().tabs.get().active().clone();
    origin.archive_path.set(Some(PathBuf::from("origin.zip")));

    let (started_tx, started_rx) = mpsc::channel();
    let (gate_tx, gate_rx) = mpsc::channel();
    let io = Arc::new(BlockingReadIo {
        started: started_tx,
        gate: Arc::new(StdMutex::new(gate_rx)),
        content: "origin content".to_string(),
    });
    let (returned_tx, returned_rx) = mpsc::channel();
    let caller_shared = shared.clone();
    let caller_origin = origin.clone();
    let caller = thread::spawn(move || {
        FileOpsService.read_text_with_io(
            &caller_shared,
            caller_origin,
            "origin.txt".to_string(),
            io,
        );
        returned_tx.send(()).unwrap();
    });

    returned_rx
        .recv_timeout(Duration::from_millis(500))
        .expect("read_text_with_io blocked its caller on archive I/O");
    assert!(matches!(
        origin.file_edit_dialog.get().load_state,
        FileEditLoadState::Loading { .. }
    ));
    assert_eq!(
        started_rx.recv_timeout(Duration::from_millis(500)).unwrap(),
        "origin.txt"
    );
    caller.join().unwrap();

    let mut tabs = shared.signals().tabs.get();
    tabs.open(Some(PathBuf::from("other.zip")));
    let other = tabs.active().clone();
    other.file_edit_dialog.update(|dialog| {
        dialog.show = true;
        dialog.full_path_in_archive = "other.txt".to_string();
        dialog.content = "other content".to_string();
    });
    shared.signals().tabs.set(tabs);

    gate_tx.send(()).unwrap();
    wait_until("text completion did not update its origin", || {
        matches!(
            origin.file_edit_dialog.get().load_state,
            FileEditLoadState::Ready
        )
    });

    assert_eq!(origin.file_edit_dialog.get().content, "origin content");
    assert_eq!(other.file_edit_dialog.get().content, "other content");
}

#[test]
fn archive_file_jobs_stale_text_completion_cannot_overwrite_newer_read() {
    let ctx = TestContext::new();
    let shared = ctx.shared.clone();
    let origin = shared.signals().tabs.get().active().clone();
    origin.archive_path.set(Some(PathBuf::from("origin.zip")));

    let (started_tx, started_rx) = mpsc::channel();
    let (gate_a_tx, gate_a_rx) = mpsc::channel();
    FileOpsService.read_text_with_io(
        &shared,
        origin.clone(),
        "a.txt".to_string(),
        Arc::new(BlockingReadIo {
            started: started_tx.clone(),
            gate: Arc::new(StdMutex::new(gate_a_rx)),
            content: "stale A".to_string(),
        }),
    );
    assert_eq!(
        started_rx.recv_timeout(Duration::from_millis(500)).unwrap(),
        "a.txt"
    );

    let (gate_b_tx, gate_b_rx) = mpsc::channel();
    FileOpsService.read_text_with_io(
        &shared,
        origin.clone(),
        "b.txt".to_string(),
        Arc::new(BlockingReadIo {
            started: started_tx,
            gate: Arc::new(StdMutex::new(gate_b_rx)),
            content: "fresh B".to_string(),
        }),
    );
    assert_eq!(
        started_rx.recv_timeout(Duration::from_millis(500)).unwrap(),
        "b.txt"
    );

    gate_b_tx.send(()).unwrap();
    wait_until("newer text read did not complete", || {
        origin.file_edit_dialog.get().content == "fresh B"
    });
    assert_eq!(origin.file_edit_dialog.get().full_path_in_archive, "b.txt");

    gate_a_tx.send(()).unwrap();
    wait_until("older text read did not finish", || {
        origin.in_flight_ops.load(Ordering::SeqCst) == 0
    });

    let dialog = origin.file_edit_dialog.get();
    assert_eq!(dialog.full_path_in_archive, "b.txt");
    assert_eq!(dialog.content, "fresh B");
    assert_eq!(dialog.original_content, "fresh B");
    assert_eq!(dialog.load_state, FileEditLoadState::Ready);
}

#[test]
fn archive_file_jobs_ready_text_survives_stale_rendered_loading_snapshot() {
    let mut current = FileEditDialog {
        show: true,
        full_path_in_archive: "entry.txt".to_string(),
        name_input: "entry.txt".to_string(),
        content: "worker content".to_string(),
        original_content: "worker content".to_string(),
        error: String::new(),
        load_state: FileEditLoadState::Ready,
    };
    let rendered_before_completion = FileEditDialog {
        show: true,
        full_path_in_archive: "entry.txt".to_string(),
        name_input: "entry.txt".to_string(),
        content: String::new(),
        original_content: String::new(),
        error: String::new(),
        load_state: FileEditLoadState::Loading { request_id: 7 },
    };

    current.apply_rendered_snapshot(rendered_before_completion);

    assert_eq!(current.content, "worker content");
    assert_eq!(current.original_content, "worker content");
    assert_eq!(current.load_state, FileEditLoadState::Ready);
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
        signals.tabs.get().active().listing.get().current_path(),
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
        signals.tabs.get().active().listing.get().current_path(),
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
            archive_path: "test.txt".to_string(),
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

    let tab = signals.tabs.get().active().clone();
    tab.listing.update(|listing| {
        assert!(listing.go_to("root/folder"));
    });

    let filename = "file.txt".to_string();
    ctx.handle_action(Action::CopyPath(filename.clone()));

    tab.listing.update(|listing| {
        assert!(listing.go_to(""));
    });
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

// `Action::Metadata` and the controller arm that parsed a plugin's JSON
// into a tab's metadata are gone. Nothing ever emitted that action, and
// the arm was a second parser of the same document with different rules
// (a plain derived deserialize, so it dropped the `circle` -> `creator`
// mapping) writing to whichever tab happened to be active rather than to
// the one whose archive the fetch was for. The live arrival path --
// session write, `MetadataChanged`, tab, views -- is covered by
// `session_event_bridge_test.rs` and `organize_panel_test.rs`.

/// Organizing opens the panel for the active tab's archive *session*:
/// the panel is bound to it, so everything it previews and the organize
/// it eventually runs describe the same archive.
#[test]
fn test_organize_action() {
    let (temp, shared) = common::create_test_shared_state_with_facade();
    let app = shared
        .facade
        .as_ref()
        .expect("the fixture has a facade")
        .clone();

    // A real archive, opened the way the browser opens one.
    let archive = temp.path().join("test.zip");
    {
        let file = std::fs::File::create(&archive).expect("create zip fixture");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("a.txt", zip::write::SimpleFileOptions::default())
            .expect("start zip entry");
        std::io::Write::write_all(&mut writer, b"one").expect("write zip entry");
        writer.finish().expect("finish zip fixture");
    }
    let session_id = shared.services.tokio_runtime.block_on(async {
        let operation_id = app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: archive.clone(),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match app.operation(operation_id).await.unwrap().state {
                arclain_app::event::OperationState::Completed {
                    result: arclain_app::event::OperationResult::ArchiveOpened { snapshot },
                } => return snapshot.session_id,
                arclain_app::event::OperationState::Failed { error } => {
                    panic!("opening the fixture failed: {error:?}")
                }
                _ => {
                    assert!(Instant::now() < deadline, "open timed out");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    });

    let tab = shared.signals().tabs.get().active().clone();
    tab.archive_path.set(Some(archive));
    tab.archive_session_id.set(Some(session_id));

    let mut org_feature = arclain_ui::features::organization::OrganizationFeature::new(&shared);
    let mut navigator = arclain_ui::core::navigation::PageNavigator::new();
    let mut ops_state = ArchiveOperationsState::default();
    BrowserController::new().handle_action(
        Action::Organize,
        &shared,
        &mut ops_state,
        &mut org_feature,
        &mut navigator,
        &egui::Context::default(),
    );

    // Verify navigation
    if let arclain_ui::core::AppPage::OrganizeArchive(name) = &navigator.current_page {
        assert_eq!(name, "test.zip");
    } else {
        panic!(
            "Expected OrganizeArchive page, got {:?}",
            navigator.current_page
        );
    }

    // Verify feature state: the panel is bound to the tab's session.
    let panel = &org_feature
        .organizer_page
        .as_ref()
        .expect("the organizer page must open")
        .panel;
    assert_eq!(panel.session_id, session_id);
    assert_eq!(panel.archive_name, "test.zip");
    assert!(
        !panel.profiles.is_empty(),
        "the panel offers the application's own archive profiles"
    );
}

/// Without an open session there is nothing to organize *against*, so
/// the panel does not open at all rather than opening bound to nothing.
#[test]
fn organize_without_an_open_session_does_not_open_the_panel() {
    let (_temp, shared) = common::create_test_shared_state_with_facade();
    let tab = shared.signals().tabs.get().active().clone();
    tab.archive_path.set(Some(PathBuf::from("test.zip")));

    let mut org_feature = arclain_ui::features::organization::OrganizationFeature::new(&shared);
    let mut navigator = arclain_ui::core::navigation::PageNavigator::new();
    let mut ops_state = ArchiveOperationsState::default();
    BrowserController::new().handle_action(
        Action::Organize,
        &shared,
        &mut ops_state,
        &mut org_feature,
        &mut navigator,
        &egui::Context::default(),
    );

    assert!(org_feature.organizer_page.is_none());
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
    // The rows below are an answer, so the listing has to say one was
    // given -- otherwise this renders the "contents not known yet" panel,
    // whose spinner repaints every frame and never settles.
    settle_listing(&tab, arclain_app::ids::ArchiveSessionId::from_raw(1));
    tab.browser_entries.update(|snapshot| {
        snapshot.replace(vec![FileEntry {
            name: "test_ui_file.txt".to_string(),
            path: "test_ui_file.txt".to_string(),
            archive_path: "test_ui_file.txt".to_string(),
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
            archive_path: format!("entry-{index:05}.txt"),
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
    seed_inventory(
        &tab,
        &(0..10_000)
            .map(|index| format!("folder-{index:05}/entry.txt"))
            .collect::<Vec<_>>(),
    );
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

    let browser = Rc::new(RefCell::new(ArchiveBrowser::new(&shared)));
    let render_browser = browser.clone();
    let mut harness = Harness::new(move |ctx| {
        let _ = render_browser.borrow_mut().render(ctx, &shared);
    });
    harness.run_steps(2);

    let after = tab.browser_entries.get();
    assert!(Arc::ptr_eq(&before.entries, &after.entries));
    assert_eq!(entry_notifications.load(Ordering::SeqCst), 0);
    assert_eq!(view_notifications.load(Ordering::SeqCst), 0);
    assert_eq!(browser.borrow().tree_projection_rebuild_count(tab.id), 1);
}

#[test]
fn full_toolbar_and_browser_idle_frames_do_not_publish_browser_view_state() {
    use arclain_ui::features::archive_browser::ArchiveBrowser;
    use arclain_ui::shared::components::toolbar::{self, ToolbarConfig};
    use egui_kittest::Harness;

    let ctx = TestContext::new();
    let shared = ctx.shared.clone();
    let tab = shared.signals().tabs.get().active().clone();
    tab.archive_path.set(Some(PathBuf::from("settled.zip")));
    tab.browser_entries.update(|snapshot| {
        snapshot.replace(vec![FileEntry {
            name: "settled.txt".to_string(),
            path: "settled.txt".to_string(),
            archive_path: "settled.txt".to_string(),
            size: "0 B".to_string(),
            compressed: "0 B".to_string(),
            ratio: "0%".to_string(),
            modified: String::new(),
            crc32: String::new(),
            encrypted: false,
            is_folder: false,
        }]);
    });
    seed_inventory(&tab, &["folder/settled.txt".to_string()]);
    tab.browser_view_state.update(|state| {
        state.toolbar_state.show_tree_panel = true;
    });

    let notifications = Arc::new(AtomicUsize::new(0));
    let notifications_for_listener = notifications.clone();
    tab.browser_view_state.subscribe(move || {
        notifications_for_listener.fetch_add(1, Ordering::SeqCst);
    });

    let mut browser = ArchiveBrowser::new(&shared);
    let shared_for_frame = shared.clone();
    let tab_for_frame = tab.clone();
    let mut harness = Harness::new(move |ctx| {
        egui::TopBottomPanel::top("toolbar_panel").show(ctx, |ui| {
            let mut view_state = tab_for_frame.browser_view_state.get();
            let config = ToolbarConfig::new(shared_for_frame.signals().toolbar_items.get());
            let mut plugin_renderer = |_: &mut egui::Ui, _: &str, _: Option<&str>| {};
            let _ = toolbar::render(
                ui,
                &shared_for_frame.theme,
                &mut view_state.toolbar_state,
                false,
                false,
                false,
                true,
                false,
                false,
                Some(&config),
                Some(&shared_for_frame),
                &mut plugin_renderer,
            );
            tab_for_frame.browser_view_state.set_if_changed(view_state);
        });
        let _ = browser.render(ctx, &shared_for_frame);
    });

    harness.run_steps(2);

    assert_eq!(notifications.load(Ordering::SeqCst), 0);
}

#[test]
fn production_toolbar_uses_change_gated_browser_view_publication() {
    let source = include_str!("../src/core/arclain_app/toolbar_handler.rs");
    assert!(source.contains("tab.browser_view_state.set_if_changed(view_state);"));
    assert!(!source.contains("tab.browser_view_state.set(view_state);"));
}

/// The render layer's own pin for the browser-model cutover: the rows
/// the file list draws are produced from the session's `ArchiveEntryDto`
/// rows, and both pieces of per-entry view state that must outlive a
/// refresh -- the selection and the tree's folder expansion -- do.
///
/// `TabListing`'s and the relist pipeline's own tests pin that survival at
/// their layers; this pins it where the user actually experiences it,
/// through a real render of the real `ArchiveBrowser` between the two
/// inventories.
#[test]
fn the_rendered_browser_draws_dto_rows_and_keeps_selection_and_expansion_across_a_refresh() {
    use arclain_app::archive::EntryKind;
    use arclain_ui::features::archive_browser::ArchiveBrowser;
    use egui_kittest::Harness;

    let ctx = TestContext::new();
    let shared = ctx.shared.clone();
    let tab = shared.signals().tabs.get().active().clone();
    tab.archive_path.set(Some(PathBuf::from("fixture.zip")));

    let rows = |extra: bool| {
        let mut rows = vec![
            ("game".to_string(), EntryKind::Directory),
            ("game/Game.exe".to_string(), EntryKind::File),
            ("game/data".to_string(), EntryKind::Directory),
            ("game/data/save.dat".to_string(), EntryKind::File),
            ("readme.txt".to_string(), EntryKind::File),
        ];
        if extra {
            rows.push(("game/added.txt".to_string(), EntryKind::File));
        }
        rows
    };

    // The user is browsing `game`, has a row selected there, and has the
    // tree expanded to it.
    seed_inventory_rows(&tab, 1, &rows(false));
    tab.listing.update(|listing| {
        assert!(listing.go_to("game"));
    });
    arclain_ui::core::operations::browser_rows::publish_browsed_directory(shared.signals());
    {
        let mut view_state = tab.browser_view_state.get();
        view_state.toolbar_state.show_tree_panel = true;
        view_state.selection.insert("game/Game.exe".to_string());
        tab.browser_view_state.set(view_state);
    }

    let browser = Rc::new(RefCell::new(ArchiveBrowser::new(&shared)));
    let render_browser = browser.clone();
    let render_shared = shared.clone();
    let mut harness = Harness::new(move |ctx| {
        let _ = render_browser.borrow_mut().render(ctx, &render_shared);
    });
    harness.run();

    let drawn = tab.browser_entries.get();
    let paths: Vec<&str> = drawn
        .entries
        .iter()
        .map(|row| row.archive_path.as_str())
        .collect();
    assert_eq!(
        paths,
        ["game/Game.exe", "game/data"],
        "the file list must draw the browsed directory's own entries"
    );
    assert_eq!(
        drawn.entries[0].modified, "2024-01-15 10:30:00",
        "a rendered row's Modified cell comes from the DTO's own timestamp"
    );
    assert!(
        drawn.entries[1].is_folder,
        "the synthesized directory row must still render as a folder"
    );

    // The first render expanded the tree down to the browsed folder.
    // `TreePanelState`'s equality is its selected path plus its
    // expansion generation, so a re-expansion after the refresh (which
    // is what losing the expansion would force) shows up as inequality.
    let tree_state_before = tab.browser_view_state.get().tree_state.clone();

    // A refresh: the session answers with a higher revision carrying one
    // more file, exactly as a mutation relist would.
    seed_inventory_rows(&tab, 2, &rows(true));
    arclain_ui::core::operations::browser_rows::publish_browsed_directory(shared.signals());
    harness.run();

    let refreshed = tab.browser_entries.get();
    let refreshed_paths: Vec<&str> = refreshed
        .entries
        .iter()
        .map(|row| row.archive_path.as_str())
        .collect();
    assert_eq!(
        refreshed_paths,
        ["game/Game.exe", "game/data", "game/added.txt"],
        "the refresh must reach the rendered rows"
    );

    let view_state = tab.browser_view_state.get();
    assert!(
        view_state.selection.contains("game/Game.exe"),
        "the selection did not survive the refresh"
    );
    assert_eq!(
        view_state.tree_state, tree_state_before,
        "the tree's folder expansion did not survive the refresh"
    );
    assert_eq!(
        view_state.tree_state.selected_path, "game",
        "the tree must still be pointing at the browsed folder"
    );
    assert_eq!(
        tab.selection_count.get(),
        1,
        "the surviving selection must still be counted for the toolbar"
    );
}
