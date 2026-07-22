use arclain_ui::features::archive_browser::application::file_ops_service::{
    ArchiveFileIo, DeleteListResult,
};
use arclain_ui::features::archive_browser::application::FileOpsService;
use arclain_ui::features::archive_browser::{Action, BrowserController};
use arclain_ui::features::archive_operations::ArchiveOperationsState;
use arclain_ui::features::file_editing::domain::types::{FileEditDialog, FileEditLoadState};
use arclain_ui::shared::models::file_entry::FileEntry;

use std::cell::RefCell;
use std::path::Path;
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

struct BlockingDeleteIo {
    started: Sender<()>,
    gate: StdMutex<Receiver<()>>,
}

impl ArchiveFileIo for BlockingDeleteIo {
    fn delete_and_list(
        &self,
        _archive: &Path,
        _paths: &[String],
    ) -> anyhow::Result<DeleteListResult> {
        self.started.send(()).unwrap();
        self.gate.lock().unwrap().recv().unwrap();
        Ok(DeleteListResult {
            archive_entries: Arc::new(vec![archive_entry("updated.txt")]),
            browser_entries: vec![file_entry("updated.txt")],
        })
    }

    fn read_text(&self, _archive: &Path, _path: &str) -> anyhow::Result<String> {
        unreachable!("delete fake must not service text reads")
    }
}

struct BlockingReadIo {
    started: Sender<String>,
    gate: StdMutex<Receiver<()>>,
    content: String,
}

struct OrderedDeleteIo {
    calls: AtomicUsize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    started: Sender<String>,
    first_gate: StdMutex<Receiver<()>>,
    second_gate: StdMutex<Receiver<()>>,
}

impl ArchiveFileIo for OrderedDeleteIo {
    fn delete_and_list(
        &self,
        _archive: &Path,
        paths: &[String],
    ) -> anyhow::Result<DeleteListResult> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let now_in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight
            .fetch_max(now_in_flight, Ordering::SeqCst);
        self.started.send(paths[0].clone()).unwrap();

        if call == 0 {
            self.first_gate.lock().unwrap().recv().unwrap();
        } else {
            self.second_gate.lock().unwrap().recv().unwrap();
        }

        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let published_name = if call == 0 {
            "after-first.txt"
        } else {
            "after-second.txt"
        };
        Ok(DeleteListResult {
            archive_entries: Arc::new(vec![archive_entry(published_name)]),
            browser_entries: vec![file_entry(published_name)],
        })
    }

    fn read_text(&self, _archive: &Path, _path: &str) -> anyhow::Result<String> {
        unreachable!("delete fake must not service text reads")
    }
}

impl ArchiveFileIo for BlockingReadIo {
    fn delete_and_list(
        &self,
        _archive: &Path,
        _paths: &[String],
    ) -> anyhow::Result<DeleteListResult> {
        unreachable!("read fake must not service delete jobs")
    }

    fn read_text(&self, _archive: &Path, path: &str) -> anyhow::Result<String> {
        self.started.send(path.to_string()).unwrap();
        self.gate.lock().unwrap().recv().unwrap();
        Ok(self.content.clone())
    }
}

fn archive_entry(path: &str) -> arclain_core::ArchiveEntry {
    arclain_core::ArchiveEntry {
        path: path.to_string(),
        size: 1,
        packed_size: 1,
        modified: None,
        is_dir: false,
        encrypted: false,
        crc32: None,
    }
}

fn file_entry(path: &str) -> FileEntry {
    FileEntry {
        name: path.to_string(),
        path: path.to_string(),
        archive_path: path.to_string(),
        size: "1 B".to_string(),
        compressed: "1 B".to_string(),
        ratio: "100%".to_string(),
        modified: String::new(),
        crc32: String::new(),
        encrypted: false,
        is_folder: false,
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

#[test]
fn archive_file_jobs_delete_returns_before_io_and_updates_origin_tab() {
    let ctx = TestContext::new();
    let shared = ctx.shared.clone();
    let origin = shared.signals().tabs.get().active().clone();
    origin.archive_path.set(Some(PathBuf::from("origin.zip")));
    origin
        .entries
        .set(Arc::new(vec![archive_entry("original.txt")]));
    origin
        .browser_entries
        .update(|snapshot| snapshot.replace(vec![file_entry("original.txt")]));

    let (started_tx, started_rx) = mpsc::channel();
    let (gate_tx, gate_rx) = mpsc::channel();
    let io = Arc::new(BlockingDeleteIo {
        started: started_tx,
        gate: StdMutex::new(gate_rx),
    });
    let (returned_tx, returned_rx) = mpsc::channel();
    let caller_shared = shared.clone();
    let caller_origin = origin.clone();
    let caller = thread::spawn(move || {
        FileOpsService.delete_files_with_io(
            &caller_shared,
            caller_origin,
            vec!["original.txt".to_string()],
            io,
        );
        returned_tx.send(()).unwrap();
    });

    returned_rx
        .recv_timeout(Duration::from_millis(500))
        .expect("delete_files_with_io blocked its caller on archive I/O");
    started_rx
        .recv_timeout(Duration::from_millis(500))
        .expect("delete worker did not reach fake I/O");
    caller.join().unwrap();

    let mut tabs = shared.signals().tabs.get();
    tabs.open(Some(PathBuf::from("other.zip")));
    let other = tabs.active().clone();
    other
        .entries
        .set(Arc::new(vec![archive_entry("other.txt")]));
    other
        .browser_entries
        .update(|snapshot| snapshot.replace(vec![file_entry("other.txt")]));
    shared.signals().tabs.set(tabs);

    gate_tx.send(()).unwrap();
    wait_until("delete completion did not update its origin", || {
        origin.entries.get()[0].path == "updated.txt"
    });

    assert_eq!(origin.browser_entries.get().entries[0].name, "updated.txt");
    assert_eq!(other.entries.get()[0].path, "other.txt");
    assert_eq!(other.browser_entries.get().entries[0].name, "other.txt");
}

#[test]
fn archive_file_jobs_serialized_deletes_publish_before_next_edit_runs() {
    let ctx = TestContext::new();
    let shared = ctx.shared.clone();
    let origin = shared.signals().tabs.get().active().clone();
    origin.archive_path.set(Some(PathBuf::from("origin.zip")));
    origin
        .entries
        .set(Arc::new(vec![archive_entry("before.txt")]));
    origin
        .browser_entries
        .update(|snapshot| snapshot.replace(vec![file_entry("before.txt")]));

    let (started_tx, started_rx) = mpsc::channel();
    let (first_gate_tx, first_gate_rx) = mpsc::channel();
    let (second_gate_tx, second_gate_rx) = mpsc::channel();
    let io = Arc::new(OrderedDeleteIo {
        calls: AtomicUsize::new(0),
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
        started: started_tx,
        first_gate: StdMutex::new(first_gate_rx),
        second_gate: StdMutex::new(second_gate_rx),
    });

    FileOpsService.delete_files_with_io(
        &shared,
        origin.clone(),
        vec!["first.txt".to_string()],
        io.clone(),
    );
    assert_eq!(
        started_rx.recv_timeout(Duration::from_millis(500)).unwrap(),
        "first.txt"
    );

    FileOpsService.delete_files_with_io(
        &shared,
        origin.clone(),
        vec!["second.txt".to_string()],
        io.clone(),
    );
    wait_until("second delete was not scheduled", || {
        origin.in_flight_ops.load(Ordering::SeqCst) == 2
    });
    assert!(
        started_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "the second backend edit overlapped the blocked first edit"
    );

    first_gate_tx.send(()).unwrap();
    assert_eq!(
        started_rx.recv_timeout(Duration::from_millis(500)).unwrap(),
        "second.txt"
    );
    let first_published_before_second = origin.entries.get()[0].path == "after-first.txt";

    second_gate_tx.send(()).unwrap();
    wait_until("serialized deletes did not complete", || {
        origin.in_flight_ops.load(Ordering::SeqCst) == 0
    });

    assert_eq!(io.max_in_flight.load(Ordering::SeqCst), 1);
    assert!(
        first_published_before_second,
        "the first edit released serialization before publishing its snapshot"
    );
    assert_eq!(origin.entries.get()[0].path, "after-second.txt");
    assert_eq!(
        origin.browser_entries.get().entries[0].name,
        "after-second.txt"
    );
}

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
        gate: StdMutex::new(gate_rx),
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
            gate: StdMutex::new(gate_a_rx),
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
            gate: StdMutex::new(gate_b_rx),
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
    tab.entries.set(Arc::new(
        (0..10_000)
            .map(|index| arclain_core::ArchiveEntry {
                path: format!("folder-{index:05}/entry.txt"),
                size: 0,
                packed_size: 0,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            })
            .collect(),
    ));
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
    tab.entries.set(Arc::new(vec![arclain_core::ArchiveEntry {
        path: "folder/settled.txt".to_string(),
        size: 0,
        packed_size: 0,
        modified: None,
        is_dir: false,
        encrypted: false,
        crc32: None,
    }]));
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
            let mut plugin_renderer = |_: &mut egui::Ui, _: &str, _: &_| Vec::new();
            let mut plugin_dispatcher = |_: String, _: String, _: Option<String>| {};
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
                &mut plugin_dispatcher,
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
