use crate::protocol::*;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use unboxer::{EntryStream, Inventory, SequentialArchive, UnboxerError};

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct WorkerOptions {
    pub path: PathBuf,
    pub memory_bytes: u64,
    /// How long the worker may go without a frame while a request is
    /// outstanding before it is killed. A request stays outstanding until
    /// the worker has exited after COMPLETE, so this must exceed the
    /// worker's exit latency: its heartbeat thread joins within 250 ms.
    pub stall_timeout: Duration,
    /// Bytes of entry data per second the worker must average while an
    /// entry streams, between its BEGIN and its END, measured over windows
    /// of `stall_timeout` spent waiting on it, after the first such window
    /// of a decode. Zero disables the floor.
    pub min_decode_rate: u64,
}
impl WorkerOptions {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            // Half the machine, within a floor a game archive needs and a
            // ceiling past which a decoder is asking for something else.
            memory_bytes: physical_memory()
                .map_or(1 << 30, |ram| (ram / 2).clamp(1 << 30, 8 << 30)),
            stall_timeout: Duration::from_secs(60),
            min_decode_rate: 1 << 20,
        }
    }
}

/// The machine's physical memory, or `None` where it cannot be read.
fn physical_memory() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        // SAFETY: the structure is sized and zeroed as the call requires.
        (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullTotalPhys)
    }
    #[cfg(unix)]
    {
        // SAFETY: sysconf takes no pointers.
        let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        (pages > 0 && page > 0).then(|| pages as u64 * page as u64)
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

/// What the reader thread has seen of the worker, read by the watchdog.
/// Time spent inside `read_frame` is time the worker is the bottleneck;
/// time the reader spends handing a frame to a slow consumer is not.
struct Liveness {
    started: Instant,
    /// Whether a request is outstanding: from the spawn to INVENTORY_END,
    /// and from DECODE to the worker's exit after COMPLETE, or to FAILURE.
    outstanding: AtomicBool,
    /// Milliseconds since `started` at which the outstanding request was
    /// sent. Nothing before it counts against the worker: between requests
    /// the reader waits on a worker that is idle, not stalled.
    outstanding_since_ms: AtomicU64,
    /// Milliseconds since `started` at which the current `read_frame`
    /// began, or `u64::MAX` when the reader is not waiting on the worker.
    waiting_since_ms: AtomicU64,
    /// Whether the rate floor applies: from DECODE to COMPLETE or FAILURE.
    decoding: AtomicBool,
    /// Milliseconds spent waiting on the worker, and bytes of DATA payload
    /// received, while an entry streams: between a BEGIN and its END. The
    /// seek to the next entry and the source re-hash before COMPLETE owe
    /// no rate; heartbeats keep them under the stall rule instead.
    decode_wait_ms: AtomicU64,
    decode_bytes: AtomicU64,
    /// Set by the watchdog before it kills the worker for a stall or for
    /// trickling, so the failure is reported as `TimedOut`.
    stalled: AtomicBool,
}
impl Liveness {
    /// READY answers the spawn, so a request is outstanding and the reader
    /// counts as waiting from the start.
    fn new() -> Self {
        Self {
            started: Instant::now(),
            outstanding: AtomicBool::new(true),
            outstanding_since_ms: AtomicU64::new(0),
            waiting_since_ms: AtomicU64::new(0),
            decoding: AtomicBool::new(false),
            decode_wait_ms: AtomicU64::new(0),
            decode_bytes: AtomicU64::new(0),
            stalled: AtomicBool::new(false),
        }
    }
    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
    /// DECODE is about to be sent. The mark is stored before the flags so
    /// that whoever sees a flag set also sees the mark.
    fn decode_sent(&self) {
        self.outstanding_since_ms
            .store(self.now_ms(), Ordering::Release);
        self.decoding.store(true, Ordering::Release);
        self.outstanding.store(true, Ordering::Release);
    }
    /// The worker has answered the outstanding request.
    fn answered(&self) {
        self.decoding.store(false, Ordering::Release);
        self.outstanding.store(false, Ordering::Release);
    }
}

#[derive(Clone)]
pub struct ArchiveClient {
    options: WorkerOptions,
}
impl ArchiveClient {
    pub fn new(options: WorkerOptions) -> Self {
        Self { options }
    }
    pub fn open(
        &self,
        path: &Path,
        limits: unboxer::Limits,
        cancel: Cancellation,
    ) -> Result<ArchiveSession, ImportError> {
        let mut command = Command::new(&self.options.path);
        command
            .args(["--memory-limit", &self.options.memory_bytes.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|_| ImportError::Unavailable)?;
        let stdin = child.stdin.take().ok_or(ImportError::Io)?;
        let mut stdout = child.stdout.take().ok_or(ImportError::Io)?;
        let mut stderr = child.stderr.take().ok_or(ImportError::Io)?;
        let child = Arc::new(Mutex::new(child));
        let (tx, rx) = mpsc::sync_channel(2);
        let liveness = Arc::new(Liveness::new());
        let reader_liveness = liveness.clone();
        let reader = thread::spawn(move || {
            // BEGIN and END both pass through here, so the reader itself
            // knows when an entry is streaming, and only then does the wait
            // for a frame, or the payload of one, count towards the rate.
            let mut streaming = false;
            loop {
                let began = reader_liveness.now_ms();
                reader_liveness
                    .waiting_since_ms
                    .store(began, Ordering::Release);
                let value = read_frame(&mut stdout);
                reader_liveness
                    .waiting_since_ms
                    .store(u64::MAX, Ordering::Release);
                // Accounted before the send below, which may block on a slow
                // consumer for as long as it likes through no fault of the
                // worker. Bytes first: the watchdog reads the wait before the
                // bytes, so the window it is closing is never judged on
                // waiting time whose bytes it has not yet seen. The other
                // tear remains: the watchdog can take a frame's bytes into
                // the closing window without its wait, and the next window
                // then carries that wait without its bytes. The order
                // guarantees only that the current window is never judged
                // strict.
                if streaming {
                    if let Ok((DATA, data)) = &value {
                        reader_liveness
                            .decode_bytes
                            .fetch_add(data.len() as u64, Ordering::AcqRel);
                    }
                    let waited = reader_liveness.now_ms().saturating_sub(began);
                    reader_liveness
                        .decode_wait_ms
                        .fetch_add(waited, Ordering::AcqRel);
                }
                match &value {
                    Ok((BEGIN, _)) => streaming = true,
                    Ok((END, _)) => streaming = false,
                    _ => {}
                }
                // A heartbeat only refreshes the wait; nothing downstream sees
                // it. One with a payload breaks the protocol and is passed on
                // to be refused like any other unexpected frame.
                if matches!(&value, Ok((HEARTBEAT, data)) if data.is_empty()) {
                    continue;
                }
                let failed = value.is_err();
                if tx.send(value).is_err() || failed {
                    break;
                }
            }
        });
        let errors = thread::spawn(move || {
            let mut buf = [0; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
        });
        let done = Arc::new(AtomicBool::new(false));
        let stall_ms = u64::try_from(self.options.stall_timeout.as_millis()).unwrap_or(u64::MAX);
        let min_rate = self.options.min_decode_rate;
        let guard_child = child.clone();
        let guard_done = done.clone();
        let guard_cancel = cancel.clone();
        let guard_liveness = liveness.clone();
        // A blocked pipe write is covered as well: while the caller is stuck
        // writing a request, the reader is waiting on the worker, so the stall
        // rule kills a worker that does not read. That fails the write, whose
        // error reaches the caller as `Io`: nothing on that path consults
        // `stalled`, so the kill is not reported as `TimedOut`.
        let watchdog = thread::spawn(move || {
            // The rate window in progress: the waiting time at which it
            // closes, and the waiting time and bytes at the previous close.
            let mut window_end_ms = stall_ms;
            let mut window_wait_ms = 0u64;
            let mut window_bytes = 0u64;
            let mut first_window = true;
            while !guard_done.load(Ordering::Acquire) {
                let now = guard_liveness.now_ms();
                let outstanding = guard_liveness.outstanding.load(Ordering::Acquire);
                let waiting = guard_liveness.waiting_since_ms.load(Ordering::Acquire);
                let since = guard_liveness.outstanding_since_ms.load(Ordering::Acquire);
                let stalled = outstanding
                    && waiting != u64::MAX
                    && now.saturating_sub(waiting.max(since)) >= stall_ms;
                let mut trickling = false;
                if guard_liveness.decoding.load(Ordering::Acquire) {
                    let wait = guard_liveness.decode_wait_ms.load(Ordering::Acquire);
                    if wait >= window_end_ms {
                        let bytes = guard_liveness.decode_bytes.load(Ordering::Acquire);
                        // Judged over the window's own waiting time, which a
                        // long read and the poll interval stretch past stall_ms.
                        let floor =
                            min_rate.saturating_mul(wait.saturating_sub(window_wait_ms)) / 1000;
                        trickling = !first_window && bytes.saturating_sub(window_bytes) < floor;
                        first_window = false;
                        window_wait_ms = wait;
                        window_bytes = bytes;
                        window_end_ms = wait.saturating_add(stall_ms);
                    }
                }
                if guard_cancel.is_cancelled() || stalled || trickling {
                    if stalled || trickling {
                        guard_liveness.stalled.store(true, Ordering::Release);
                    }
                    if let Ok(mut child) = guard_child.lock() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let mut session = ArchiveSession {
            child,
            stdin,
            rx: Some(rx),
            reader: Some(reader),
            errors: Some(errors),
            watchdog: Some(watchdog),
            done,
            cancel,
            liveness,
            inventory: Inventory {
                container: unboxer::Container::Unknown,
                entries: vec![],
                blocks: vec![],
            },
            started: false,
            finished: false,
            active: None,
            last_error: None,
        };
        let (tag, data) = session.receive()?;
        if tag != READY || data != b"arclain-import-v1" {
            return Err(ImportError::Corrupt);
        }
        json(
            &mut session.stdin,
            OPEN,
            &Request {
                source: path.to_path_buf(),
                limits: limits.into(),
            },
        )?;
        let (tag, data) = session.receive()?;
        if tag != ARCHIVE {
            return Err(ImportError::Corrupt);
        }
        session.inventory.container = parse(&data)?;
        let mut metadata = 0u64;
        loop {
            let (tag, data) = session.receive()?;
            metadata = metadata
                .checked_add(data.len() as u64)
                .ok_or(ImportError::Limit)?;
            if metadata > limits.max_index_bytes {
                return Err(ImportError::Limit);
            }
            match tag {
                BLOCK => {
                    if !session.inventory.entries.is_empty()
                        || session.inventory.blocks.len() as u64 >= limits.max_entries
                    {
                        return Err(ImportError::Limit);
                    }
                    session.inventory.blocks.push(parse(&data)?);
                }
                ENTRY => {
                    if session.inventory.entries.len() as u64 >= limits.max_entries {
                        return Err(ImportError::Limit);
                    }
                    session.inventory.entries.push(parse(&data)?);
                }
                INVENTORY_END => {
                    if index(&data)? != session.inventory.entries.len() {
                        return Err(ImportError::Corrupt);
                    }
                    break;
                }
                _ => return Err(ImportError::Corrupt),
            }
        }
        session.liveness.answered();
        // Storage policy runs in Unboxer at the caller, but malformed inventory
        // identities must also be refused before exposing any stream.
        if session
            .inventory
            .entries
            .iter()
            .enumerate()
            .any(|(i, e)| i != e.index)
        {
            return Err(ImportError::Corrupt);
        }
        Ok(session)
    }
}

type WorkerFrame = Result<(u8, Vec<u8>), ImportError>;

pub struct ArchiveSession {
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
    rx: Option<mpsc::Receiver<WorkerFrame>>,
    reader: Option<JoinHandle<()>>,
    errors: Option<JoinHandle<()>>,
    watchdog: Option<JoinHandle<()>>,
    done: Arc<AtomicBool>,
    cancel: Cancellation,
    liveness: Arc<Liveness>,
    inventory: Inventory,
    started: bool,
    finished: bool,
    active: Option<usize>,
    last_error: Option<ImportError>,
}
impl ArchiveSession {
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }
    /// Recover the precise worker failure after the format-neutral reader
    /// returns an I/O or decoder error. Storage failures do not set this.
    pub fn take_error(&mut self) -> Option<ImportError> {
        self.last_error.take()
    }
    pub fn process_id(&self) -> u32 {
        self.child.lock().unwrap().id()
    }
    /// Why the caller should stop waiting on the worker, if it should:
    /// cancellation first, then the watchdog having killed it for a stall.
    fn interrupted(&self) -> Option<ImportError> {
        if self.cancel.is_cancelled() {
            Some(ImportError::Cancelled)
        } else if self.liveness.stalled.load(Ordering::Acquire) {
            Some(ImportError::TimedOut)
        } else {
            None
        }
    }
    fn receive(&mut self) -> Result<(u8, Vec<u8>), ImportError> {
        let result = self.receive_inner();
        if let Err(error) = result {
            self.last_error = Some(error);
        }
        result
    }
    fn receive_inner(&mut self) -> Result<(u8, Vec<u8>), ImportError> {
        loop {
            if let Some(error) = self.interrupted() {
                return Err(error);
            }
            match self
                .rx
                .as_ref()
                .ok_or(ImportError::Corrupt)?
                .recv_timeout(Duration::from_millis(25))
            {
                Ok(Ok((FAILURE, data))) => {
                    self.liveness.answered();
                    return Err(parse(&data)?);
                }
                Ok(Ok(frame)) => return Ok(frame),
                Ok(Err(error)) => {
                    return Err(self.interrupted().unwrap_or(if error == ImportError::Io {
                        ImportError::WorkerFailed
                    } else {
                        error
                    }))
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self.interrupted().unwrap_or(ImportError::WorkerFailed))
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    fn next(&mut self) -> Result<Option<usize>, ImportError> {
        if let Some(error) = self.interrupted() {
            return Err(error);
        }
        if self.active.is_some() {
            return Err(ImportError::Corrupt);
        }
        if self.finished {
            return Ok(None);
        }
        if !self.started {
            self.liveness.decode_sent();
            frame(&mut self.stdin, DECODE, &[])?;
            self.started = true;
        }
        let (tag, data) = self.receive()?;
        if tag == COMPLETE {
            if !data.is_empty() {
                return Err(ImportError::Corrupt);
            }
            // COMPLETE ends the decode, and with it the rate floor. The
            // request stays outstanding until the process has exited:
            // completion requires a successful exit, and the stall rule
            // bounds a child that hangs on its way out.
            self.liveness.decoding.store(false, Ordering::Release);
            loop {
                let result = self.child.lock().map_err(|_| ImportError::Io)?.try_wait()?;
                if let Some(status) = result {
                    if !status.success() {
                        return Err(self.interrupted().unwrap_or(ImportError::WorkerFailed));
                    }
                    break;
                }
                if let Some(error) = self.interrupted() {
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(10));
            }
            self.liveness.answered();
            self.finished = true;
            self.done.store(true, Ordering::Release);
            return Ok(None);
        }
        if tag != BEGIN {
            return Err(ImportError::Corrupt);
        }
        let index = index(&data)?;
        if index >= self.inventory.entries.len() {
            return Err(ImportError::Corrupt);
        }
        self.active = Some(index);
        Ok(Some(index))
    }
}
struct Contents<'a> {
    session: &'a mut ArchiveSession,
    pending: Vec<u8>,
    offset: usize,
    ended: bool,
}
impl Contents<'_> {
    fn read_contents(&mut self, buffer: &mut [u8]) -> Result<usize, ImportError> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.ended {
            return Ok(0);
        }
        if self.offset == self.pending.len() {
            let (tag, data) = self.session.receive()?;
            if tag == END {
                if Some(index(&data)?) != self.session.active {
                    return Err(ImportError::Corrupt);
                }
                self.session.active = None;
                self.ended = true;
                return Ok(0);
            }
            if tag != DATA || data.is_empty() {
                return Err(ImportError::Corrupt);
            }
            self.pending = data;
            self.offset = 0;
        }
        let n = buffer.len().min(self.pending.len() - self.offset);
        buffer[..n].copy_from_slice(&self.pending[self.offset..self.offset + n]);
        self.offset += n;
        Ok(n)
    }
}
impl Read for Contents<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.read_contents(buffer).map_err(|error| {
            self.session.last_error = Some(error);
            io::Error::other(error)
        })
    }
}
impl SequentialArchive for ArchiveSession {
    fn inventory(&self) -> &Inventory {
        &self.inventory
    }
    fn next_entry(&mut self) -> Result<Option<EntryStream<'_>>, UnboxerError> {
        self.next()
            .map_err(|error| {
                self.last_error = Some(error);
                UnboxerError::Corrupt
            })
            .map(|index| {
                index.map(|index| EntryStream {
                    index,
                    reader: Box::new(Contents {
                        session: self,
                        pending: vec![],
                        offset: 0,
                        ended: false,
                    }),
                })
            })
    }
}
impl Drop for ArchiveSession {
    fn drop(&mut self) {
        // Release the channel before joining: the reader may be blocked sending a
        // bounded frame while the caller is abandoning a sink or validation error.
        self.rx.take();
        if let Ok(mut child) = self.child.lock() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        self.done.store(true, Ordering::Release);
        for handle in [&mut self.reader, &mut self.errors, &mut self.watchdog] {
            if let Some(handle) = handle.take() {
                let _ = handle.join();
            }
        }
    }
}
