use crate::protocol::*;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
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
    pub timeout: Duration,
}
impl WorkerOptions {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            memory_bytes: 1 << 30,
            timeout: Duration::from_secs(600),
        }
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
        let reader = thread::spawn(move || loop {
            let value = read_frame(&mut stdout);
            let failed = value.is_err();
            if tx.send(value).is_err() || failed {
                break;
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
        let deadline = Instant::now() + self.options.timeout;
        let guard_child = child.clone();
        let guard_done = done.clone();
        let guard_cancel = cancel.clone();
        // Covers pipe writes as well as reads: even a wedged worker cannot
        // retain the caller forever while it is sending its source request.
        let watchdog = thread::spawn(move || {
            while !guard_done.load(Ordering::Acquire) {
                if guard_cancel.is_cancelled() || Instant::now() >= deadline {
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
            deadline,
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
    deadline: Instant,
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
    fn receive(&mut self) -> Result<(u8, Vec<u8>), ImportError> {
        let result = self.receive_inner();
        if let Err(error) = result {
            self.last_error = Some(error);
        }
        result
    }
    fn receive_inner(&mut self) -> Result<(u8, Vec<u8>), ImportError> {
        loop {
            if self.cancel.is_cancelled() {
                return Err(ImportError::Cancelled);
            }
            if Instant::now() >= self.deadline {
                return Err(ImportError::TimedOut);
            }
            match self
                .rx
                .as_ref()
                .ok_or(ImportError::Corrupt)?
                .recv_timeout(Duration::from_millis(25))
            {
                Ok(Ok((FAILURE, data))) => return Err(parse(&data)?),
                Ok(Ok(frame)) => return Ok(frame),
                Ok(Err(error)) => {
                    return Err(if self.cancel.is_cancelled() {
                        ImportError::Cancelled
                    } else if Instant::now() >= self.deadline {
                        ImportError::TimedOut
                    } else if error == ImportError::Io {
                        ImportError::WorkerFailed
                    } else {
                        error
                    })
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(ImportError::WorkerFailed),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    fn next(&mut self) -> Result<Option<usize>, ImportError> {
        if self.cancel.is_cancelled() {
            return Err(ImportError::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(ImportError::TimedOut);
        }
        if self.active.is_some() {
            return Err(ImportError::Corrupt);
        }
        if self.finished {
            return Ok(None);
        }
        if !self.started {
            frame(&mut self.stdin, DECODE, &[])?;
            self.started = true;
        }
        let (tag, data) = self.receive()?;
        if tag == COMPLETE {
            if !data.is_empty() {
                return Err(ImportError::Corrupt);
            }
            // Completion also requires successful process exit. The watchdog
            // remains live until that verdict, including a child hanging here.
            loop {
                let result = self.child.lock().map_err(|_| ImportError::Io)?.try_wait()?;
                if let Some(status) = result {
                    if !status.success() {
                        return Err(ImportError::WorkerFailed);
                    }
                    break;
                }
                if self.cancel.is_cancelled() {
                    return Err(ImportError::Cancelled);
                }
                if Instant::now() >= self.deadline {
                    return Err(ImportError::TimedOut);
                }
                thread::sleep(Duration::from_millis(10));
            }
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
