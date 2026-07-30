use arclain_ui::shared::components::logs_page::{
    render_file_lines, LogFileEntry, LogFileSnapshot, LogIo, LogSession, LogsPage, LogsPageState,
    LOG_ROW_HEIGHT,
};
use arclain_ui::shared::theme::AppTheme;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const EXPECTED_MAX_LOG_BYTES: u64 = 512 * 1024;

#[derive(Default)]
struct CountingLogIo {
    app_lists: AtomicUsize,
    plugin_lists: AtomicUsize,
    file_reads: AtomicUsize,
    session_reads: AtomicUsize,
    plugin_session_reads: AtomicUsize,
    completed_reads: AtomicUsize,
    requested_limits: Mutex<Vec<u64>>,
}

impl CountingLogIo {
    fn snapshot(path: &Path) -> LogFileSnapshot {
        LogFileSnapshot {
            path: path.to_path_buf(),
            lines: vec!["2026-07-22 INFO ready".to_string()],
            truncated: false,
        }
    }

    fn total_reads(&self) -> usize {
        self.file_reads.load(Ordering::SeqCst)
            + self.session_reads.load(Ordering::SeqCst)
            + self.plugin_session_reads.load(Ordering::SeqCst)
    }
}

impl LogIo for CountingLogIo {
    fn list_app_files(&self, _dir: &Path) -> io::Result<Vec<LogFileEntry>> {
        self.app_lists.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }

    fn list_plugin_files(&self, _dir: &Path) -> io::Result<Vec<LogFileEntry>> {
        self.plugin_lists.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }

    fn read_file(&self, path: &Path, max_bytes: u64) -> io::Result<LogFileSnapshot> {
        self.file_reads.fetch_add(1, Ordering::SeqCst);
        self.requested_limits.lock().unwrap().push(max_bytes);
        self.completed_reads.fetch_add(1, Ordering::SeqCst);
        Ok(Self::snapshot(path))
    }

    fn read_session(
        &self,
        path: &Path,
        _offset: u64,
        max_bytes: u64,
    ) -> io::Result<LogFileSnapshot> {
        self.session_reads.fetch_add(1, Ordering::SeqCst);
        self.requested_limits.lock().unwrap().push(max_bytes);
        self.completed_reads.fetch_add(1, Ordering::SeqCst);
        Ok(Self::snapshot(path))
    }

    fn read_plugin_session(
        &self,
        session: &LogSession,
        max_bytes: u64,
    ) -> io::Result<LogFileSnapshot> {
        self.plugin_session_reads.fetch_add(1, Ordering::SeqCst);
        self.requested_limits.lock().unwrap().push(max_bytes);
        self.completed_reads.fetch_add(1, Ordering::SeqCst);
        Ok(Self::snapshot(&session.plugin_log_dir))
    }
}

struct BlockingLogIo {
    read_started: Mutex<Option<mpsc::Sender<()>>>,
    read_release: Mutex<mpsc::Receiver<()>>,
}

impl BlockingLogIo {
    fn block(&self, path: &Path, max_bytes: u64) -> io::Result<LogFileSnapshot> {
        assert_eq!(max_bytes, EXPECTED_MAX_LOG_BYTES);
        if let Some(sender) = self.read_started.lock().unwrap().take() {
            sender.send(()).unwrap();
        }
        self.read_release.lock().unwrap().recv().unwrap();
        Ok(CountingLogIo::snapshot(path))
    }
}

impl LogIo for BlockingLogIo {
    fn list_app_files(&self, _dir: &Path) -> io::Result<Vec<LogFileEntry>> {
        Ok(Vec::new())
    }

    fn list_plugin_files(&self, _dir: &Path) -> io::Result<Vec<LogFileEntry>> {
        Ok(Vec::new())
    }

    fn read_file(&self, path: &Path, max_bytes: u64) -> io::Result<LogFileSnapshot> {
        self.block(path, max_bytes)
    }

    fn read_session(
        &self,
        path: &Path,
        _offset: u64,
        max_bytes: u64,
    ) -> io::Result<LogFileSnapshot> {
        self.block(path, max_bytes)
    }

    fn read_plugin_session(
        &self,
        session: &LogSession,
        max_bytes: u64,
    ) -> io::Result<LogFileSnapshot> {
        self.block(&session.plugin_log_dir, max_bytes)
    }
}

fn session() -> LogSession {
    LogSession::capture_with_app_offset(
        PathBuf::from("test-app.log"),
        0,
        PathBuf::from("test-plugin-logs"),
    )
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "condition was not met before timeout"
        );
        std::thread::yield_now();
    }
}

#[test]
fn idle_frames_reuse_catalog_and_tail_until_refresh_interval() {
    let io = Arc::new(CountingLogIo::default());
    let state = LogsPageState::with_session_and_io(session(), io.clone());
    let theme = AppTheme::new(false);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(640.0, 320.0))
        .build_ui_state(
            move |ui, state| {
                LogsPage::render_page(ui, &[], state, &theme.colors);
            },
            state,
        );

    wait_until(Duration::from_secs(1), || {
        io.app_lists.load(Ordering::SeqCst) == 1
            && io.plugin_lists.load(Ordering::SeqCst) == 1
            && io.completed_reads.load(Ordering::SeqCst) == 1
    });
    harness.run_steps(2);
    harness.run_steps(10);

    assert_eq!(io.app_lists.load(Ordering::SeqCst), 1);
    assert_eq!(io.plugin_lists.load(Ordering::SeqCst), 1);
    assert_eq!(io.total_reads(), 1);
    assert_eq!(
        io.requested_limits.lock().unwrap().as_slice(),
        &[EXPECTED_MAX_LOG_BYTES]
    );
}

#[test]
fn idle_deadline_schedules_wake_and_refreshes_after_interval() {
    let io = Arc::new(CountingLogIo::default());
    let state = LogsPageState::with_session_and_io(session(), io.clone());
    let theme = AppTheme::new(false);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(640.0, 320.0))
        .build_ui_state(
            move |ui, state| {
                LogsPage::render_page(ui, &[], state, &theme.colors);
            },
            state,
        );

    wait_until(Duration::from_secs(1), || {
        io.app_lists.load(Ordering::SeqCst) == 1
            && io.plugin_lists.load(Ordering::SeqCst) == 1
            && io.completed_reads.load(Ordering::SeqCst) == 1
    });
    harness.run();

    let repaint_delay = harness
        .output()
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .expect("root viewport output")
        .repaint_delay;
    assert!(
        repaint_delay <= Duration::from_secs(2),
        "idle log refresh has no scheduled wake: {repaint_delay:?}"
    );
    assert!(
        repaint_delay > Duration::ZERO,
        "idle log refresh requested a busy-loop repaint"
    );

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut next_repaint = repaint_delay;
    loop {
        std::thread::sleep(next_repaint + Duration::from_millis(25));
        harness.step();
        if io.app_lists.load(Ordering::SeqCst) == 2
            && io.plugin_lists.load(Ordering::SeqCst) == 2
            && io.completed_reads.load(Ordering::SeqCst) == 2
        {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "scheduled wake did not refresh the idle logs"
        );
        next_repaint = harness
            .output()
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .expect("root viewport output")
            .repaint_delay;
        if next_repaint == Duration::MAX {
            wait_until(Duration::from_secs(1), || {
                io.app_lists.load(Ordering::SeqCst) == 2
                    && io.plugin_lists.load(Ordering::SeqCst) == 2
                    && io.completed_reads.load(Ordering::SeqCst) == 2
            });
            break;
        }
        assert!(
            next_repaint <= Duration::from_secs(2),
            "refresh deadline was lost after an earlier repaint: {next_repaint:?}"
        );
    }
}

#[test]
fn render_returns_while_log_read_is_blocked() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (render_returned_tx, render_returned_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let io = Arc::new(BlockingLogIo {
        read_started: Mutex::new(Some(started_tx)),
        read_release: Mutex::new(release_rx),
    });

    std::thread::scope(|scope| {
        let render_thread = scope.spawn(move || {
            let state = LogsPageState::with_session_and_io(session(), io);
            let theme = AppTheme::new(false);
            let harness = Harness::builder()
                .with_size(egui::vec2(640.0, 320.0))
                .build_ui_state(
                    move |ui, state| {
                        LogsPage::render_page(ui, &[], state, &theme.colors);
                    },
                    state,
                );
            render_returned_tx.send(()).unwrap();
            // Keep the harness -- and with it the log worker's result
            // receiver -- alive until the main thread finishes the whole
            // handshake, the way a real app keeps the page alive across
            // frames. Dropping it as soon as render returned used to
            // destroy the worker pipeline out from under the already
            // queued (but not yet executed) session read: the worker's
            // first result send fails against the dropped receiver and
            // the worker exits before ever reaching the blocking read, so
            // on any scheduling where this thread finished before the
            // freshly spawned worker first ran, `started_rx` below
            // reported `Disconnected` with nothing actually wrong.
            let _ = done_rx.recv();
            drop(harness);
        });

        match started_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // The `started` sender lives inside the worker's own `io`
                // handle, and with the harness held alive above, the worker
                // can only be gone this early if the render thread itself
                // died -- join it and propagate its own panic as the real
                // failure instead of misreporting it as a missing read.
                match render_thread.join() {
                    Err(panic) => std::panic::resume_unwind(panic),
                    Ok(()) => panic!("render finished without the log read ever running"),
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = done_tx.send(());
                panic!("log read never started within its 1s budget");
            }
        }
        let render_returned = render_returned_rx.recv_timeout(Duration::from_millis(200));
        release_tx.send(()).unwrap();
        let _ = done_tx.send(());
        assert!(
            render_returned.is_ok(),
            "render waited for the blocked log read"
        );
    });
}

#[test]
fn ten_thousand_log_lines_render_fewer_than_one_hundred_rows() {
    let snapshot = LogFileSnapshot {
        path: PathBuf::from("large.log"),
        lines: (0..10_000)
            .map(|index| format!("line-{index:05} INFO message"))
            .collect(),
        truncated: false,
    };
    let filtered_indices: Vec<usize> = (0..snapshot.lines.len()).collect();
    let theme = AppTheme::new(false);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(640.0, 320.0))
        .build_ui(move |ui| {
            render_file_lines(
                ui,
                "large_log_scroll",
                &snapshot,
                &filtered_indices,
                false,
                &theme.colors,
            );
        });
    harness.run_steps(1);

    let rendered = harness
        .query_all_by(|node| node.value().is_some_and(|value| value.starts_with("line-")))
        .count();
    assert!(rendered > 0, "the viewport rendered no log rows");
    assert!(
        rendered < 100,
        "show_rows visited {rendered} of 10,000 log rows"
    );
}

#[test]
fn virtualized_rows_fill_bottom_without_ambient_spacing_drift() {
    let snapshot = LogFileSnapshot {
        path: PathBuf::from("spacing.log"),
        lines: (0..200)
            .map(|index| format!("line-{index:05} INFO message"))
            .collect(),
        truncated: false,
    };
    let filtered_indices: Vec<usize> = (0..snapshot.lines.len()).collect();
    let theme = AppTheme::new(false);
    let last_line = snapshot.lines.last().unwrap().clone();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(640.0, 320.0))
        .build_ui(move |ui| {
            ui.spacing_mut().item_spacing.y = 13.0;
            render_file_lines(
                ui,
                "spacing_log_scroll",
                &snapshot,
                &filtered_indices,
                true,
                &theme.colors,
            );
        });
    harness.run_steps(2);

    let last_row = harness
        .query_all_by(|node| node.value().as_deref() == Some(last_line.as_str()))
        .max_by(|left, right| left.rect().bottom().total_cmp(&right.rect().bottom()))
        .expect("auto-scroll did not render the last log row");
    let bottom_gap = 320.0 - last_row.rect().bottom();
    assert!(
        bottom_gap <= LOG_ROW_HEIGHT,
        "virtual row geometry left a {bottom_gap:.1}px gap at the bottom"
    );
}
