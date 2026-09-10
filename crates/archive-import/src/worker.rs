//! Narrow resource-bounded decoder entrypoint. No destination paths are accepted.
mod rar;
mod rar_headers;
mod resource_limits;
mod sevenz;
mod source;
mod zip;
use crate::protocol::*;
use std::{
    io::{self, Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use unboxer::{Inventory, Limits};

/// The worker's output and the moment it was last written to, held by both the
/// worker and its heartbeat thread.
struct Shared {
    out: Box<dyn Write + Send>,
    last_write: Instant,
}
/// The worker's output, shared with a thread that writes a HEARTBEAT frame
/// whenever the interval passes with nothing else written. Every frame is one
/// `write`, so the lock taken per write keeps frames whole.
struct Heartbeat {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Heartbeat {
    const INTERVAL: Duration = Duration::from_secs(5);
    fn start(writer: Box<dyn Write + Send>) -> Self {
        Self::with_interval(writer, Self::INTERVAL)
    }
    fn with_interval(writer: Box<dyn Write + Send>, interval: Duration) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            out: writer,
            last_write: Instant::now(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (beat_shared, beat_stop) = (shared.clone(), stop.clone());
        // Twenty wakes per interval is 250 ms at the worker's five seconds, and
        // that is how long a drop can wait for the thread to see the stop flag.
        let tick = (interval / 20).max(Duration::from_millis(1));
        let thread = thread::spawn(move || {
            while !beat_stop.load(Ordering::Acquire) {
                thread::sleep(tick);
                // A drop during that sleep must not let one more beat out.
                if beat_stop.load(Ordering::Acquire) {
                    return;
                }
                let Ok(mut guard) = beat_shared.lock() else {
                    return;
                };
                if guard.last_write.elapsed() >= interval {
                    if frame(&mut *guard.out, HEARTBEAT, &[]).is_err() {
                        return;
                    }
                    guard.last_write = Instant::now();
                }
            }
        });
        Self {
            shared,
            stop,
            thread: Some(thread),
        }
    }
}
impl Write for Heartbeat {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut guard = self
            .shared
            .lock()
            .map_err(|_| io::Error::other("poisoned"))?;
        guard.out.write_all(bytes)?;
        guard.last_write = Instant::now();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        let mut guard = self
            .shared
            .lock()
            .map_err(|_| io::Error::other("poisoned"))?;
        guard.out.flush()
    }
}
impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn main() -> i32 {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let memory = match arguments.as_slice() {
        [flag, number] if flag == "--memory-limit" => number.parse::<u64>().ok(),
        _ => None,
    };
    let Some(memory) = memory.filter(|n| (32 << 20..=64u64 << 30).contains(n)) else {
        return 2;
    };
    // No archive path or request is read before this succeeds.
    let _resources = match resource_limits::establish(memory) {
        Ok(guard) => guard,
        Err(_) => {
            let _ = json(
                &mut std::io::stdout().lock(),
                FAILURE,
                &ImportError::ResourceLimit,
            );
            return 3;
        }
    };
    let mut input = std::io::stdin().lock();
    let mut output = Heartbeat::start(Box::new(std::io::stdout()));
    let result = run(&mut input, &mut output);
    if let Err(error) = result {
        let _ = json(&mut output, FAILURE, &error);
        return 1;
    }
    0
}

fn run(input: &mut dyn Read, output: &mut dyn Write) -> Result<(), ImportError> {
    frame(output, READY, b"arclain-import-v1")?;
    let (tag, data) = read_frame(input)?;
    if tag != OPEN {
        return Err(ImportError::Corrupt);
    }
    let request: Request = parse(&data)?;
    let limits = request.limits.unpack();
    if limits.max_entries > 200_000 || limits.max_index_bytes > 64 << 20 {
        return Err(ImportError::Limit);
    }
    let mut source = source::Source::open(&request.source, limits.max_archive_bytes)?;
    let magic = source.magic()?;
    let mut archive = if magic.starts_with(b"PK\x03\x04") || magic.starts_with(b"PK\x05\x06") {
        Backend::Zip(zip::Archive::open(source.file.try_clone()?, &limits)?)
    } else if magic.starts_with(b"7z\xBC\xAF\x27\x1C") {
        Backend::SevenZ(Box::new(sevenz::Archive::open(
            source.file.try_clone()?,
            &limits,
        )?))
    } else if magic.starts_with(b"Rar!\x1A\x07") {
        Backend::Rar(rar::Archive::open(&mut source, &limits)?)
    } else {
        return Err(ImportError::Unsupported);
    };
    let inventory = archive.inventory();
    send_inventory(output, inventory, &limits)?;
    let (tag, data) = read_frame(input)?;
    if tag != DECODE || !data.is_empty() {
        return Err(ImportError::Corrupt);
    }
    // Refuse unsafe inventory independently of the caller too; this does not
    // confer write authority, and no output path exists in this process.
    unboxer::inspect_inventory(archive.inventory(), &limits)
        .map_err(|_| ImportError::Unsupported)?;
    archive.decode(output)?;
    source.verify()?;
    frame(output, COMPLETE, &[])?;
    Ok(())
}

enum Backend {
    Zip(zip::Archive),
    SevenZ(Box<sevenz::Archive>),
    Rar(rar::Archive),
}
impl Backend {
    fn inventory(&self) -> &Inventory {
        match self {
            Self::Zip(a) => &a.inventory,
            Self::SevenZ(a) => &a.inventory,
            Self::Rar(a) => &a.inventory,
        }
    }
    fn decode(&mut self, out: &mut dyn Write) -> Result<(), ImportError> {
        match self {
            Self::Zip(a) => a.decode(out),
            Self::SevenZ(a) => a.decode(out),
            Self::Rar(a) => a.decode(out),
        }
    }
}
fn send_inventory(
    out: &mut dyn Write,
    inventory: &Inventory,
    limits: &Limits,
) -> Result<(), ImportError> {
    if inventory.entries.len() as u64 > limits.max_entries
        || inventory.blocks.len() > inventory.entries.len()
    {
        return Err(ImportError::Limit);
    }
    json(out, ARCHIVE, &inventory.container)?;
    let mut bytes = 0u64;
    for block in &inventory.blocks {
        let payload = serde_json::to_vec(block).map_err(|_| ImportError::Corrupt)?;
        bytes += payload.len() as u64;
        if bytes > limits.max_index_bytes {
            return Err(ImportError::Limit);
        }
        frame(out, BLOCK, &payload)?;
    }
    for entry in &inventory.entries {
        let payload = serde_json::to_vec(entry).map_err(|_| ImportError::Corrupt)?;
        bytes += payload.len() as u64;
        if bytes > limits.max_index_bytes {
            return Err(ImportError::Limit);
        }
        frame(out, ENTRY, &payload)?;
    }
    frame(
        out,
        INVENTORY_END,
        &(inventory.entries.len() as u64).to_le_bytes(),
    )
}
pub(super) fn emit(
    index: usize,
    declared: u64,
    reader: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<(), ImportError> {
    frame(out, BEGIN, &(index as u64).to_le_bytes())?;
    let mut buffer = [0; MAX_FRAME];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buffer).map_err(|_| ImportError::Corrupt)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(ImportError::Limit)?;
        if total > declared {
            return Err(ImportError::Corrupt);
        }
        frame(out, DATA, &buffer[..n])?;
    }
    if total != declared {
        return Err(ImportError::Corrupt);
    }
    frame(out, END, &(index as u64).to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    type Log = Arc<Mutex<Vec<(Instant, Vec<u8>)>>>;
    /// Records what was written and when, so a test can see both the bytes and
    /// the gap the heartbeat waited out.
    struct Recorder(Log);
    impl Write for Recorder {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let mut log = self.0.lock().map_err(|_| io::Error::other("poisoned"))?;
            log.push((Instant::now(), bytes.to_vec()));
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_beat_waits_out_its_interval_and_a_write_resets_it() {
        const INTERVAL: Duration = Duration::from_millis(100);
        const BEAT: [u8; 5] = [HEARTBEAT, 0, 0, 0, 0];
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let written = || log.lock().unwrap().clone();

        let started = Instant::now();
        let mut output = Heartbeat::with_interval(Box::new(Recorder(log.clone())), INTERVAL);

        thread::sleep(Duration::from_millis(20));
        assert!(written().is_empty(), "the beat waits out a whole interval");

        // Checked 45 ms after the beat is due, and 50 ms before the next one
        // could be, so a loaded machine has room on both sides.
        thread::sleep(Duration::from_millis(130));
        let frames = written();
        assert_eq!(frames.len(), 1, "one silent interval is one beat");
        assert_eq!(frames[0].1, BEAT, "the beat is a frame with no payload");
        assert!(frames[0].0.duration_since(started) >= INTERVAL);

        let wrote_at = Instant::now();
        frame(&mut output, DATA, b"x").unwrap();

        thread::sleep(Duration::from_millis(30));
        let frames = written();
        assert_eq!(frames.len(), 2, "a write restarts the interval");
        assert_eq!(frames[1].1, [DATA, 1, 0, 0, 0, b'x']);

        thread::sleep(Duration::from_millis(120));
        let frames = written();
        assert_eq!(frames.len(), 3, "silence after a write beats again");
        assert_eq!(frames[2].1, BEAT);
        assert!(frames[2].0.duration_since(wrote_at) >= INTERVAL);

        drop(output);
        let at_drop = written().len();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(written().len(), at_drop, "a dropped heartbeat is silent");
    }
}
