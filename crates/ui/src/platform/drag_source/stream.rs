use arclain_core::backends::sevenz_cli::ProgressUpdate;
use std::io::Write;
use std::sync::mpsc::Sender;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use windows::core::{implement, Result, HRESULT};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, S_FALSE, S_OK};
use windows::Win32::System::Com::{
    ISequentialStream_Impl, IStream, IStream_Impl, STATSTG, STGTY_STREAM,
};

/// Pipe writer that sends bytes to a channel
struct PipeWriter {
    sender: SyncSender<std::result::Result<Vec<u8>, String>>,
}

impl Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // Clone data into vector and send
        // Using 64KB chunks or whatever size is passed
        let chunk = buf.to_vec();
        match self.sender.send(Ok(chunk)) {
            Ok(_) => Ok(buf.len()),
            Err(_) => Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Internal state for the stream pipe
struct StreamPipe {
    receiver: Receiver<std::result::Result<Vec<u8>, String>>,
    current_chunk: Option<Vec<u8>>,
    cursor: usize,
    /// Total bytes read from the pipe so far (simulated position)
    total_read: u64,
    // Keep handle to ensure thread lives until stream matches or is dropped?
    // Actually if we drop handle, thread detaches but continues running until it hits channel close?
    // We'll rename to _handle to suppress warning but keep it.
    _handle: Option<JoinHandle<()>>,
}

#[implement(IStream)]
pub struct ArchiveStream {
    pipe: Mutex<StreamPipe>,
    file_size: u64,
    progress_tx: Option<Sender<ProgressUpdate>>,
}

impl ArchiveStream {
    /// Create a new ArchiveStream that extracts the given entry on a background thread
    pub fn new(
        backend: Arc<dyn arclain_core::ArchiveBackend>,
        archive_path: std::path::PathBuf,
        entry_name: String,
        password: Option<String>,
        file_size: u64,
        progress_tx: Option<Sender<ProgressUpdate>>,
    ) -> Self {
        // Create a bounded channel for backpressure (e.g., 4 chunks)
        // Adjust buffer size as needed.
        let (sender, receiver) = std::sync::mpsc::sync_channel(4);

        let entry_name_clone = entry_name.clone();

        // Spawn the extraction thread
        let handle = std::thread::spawn(move || {
            let mut writer = PipeWriter {
                sender: sender.clone(),
            };

            let result = backend.extract_entry_to_writer(
                &archive_path,
                &entry_name_clone,
                password.as_deref(),
                &mut writer,
            );

            if let Err(e) = result {
                tracing::error!("Stream extraction failed for '{}': {}", entry_name_clone, e);
                // Send error to receiver
                let _ = sender.send(Err(e.to_string()));
            }
            // Logic works: sender dropped here -> channel closes -> receiver sees disconnect (EOF)
        });

        Self {
            pipe: Mutex::new(StreamPipe {
                receiver,
                current_chunk: None,
                cursor: 0,
                total_read: 0,
                _handle: Some(handle),
            }),
            file_size,
            progress_tx,
        }
    }
}

impl ISequentialStream_Impl for ArchiveStream {
    fn Read(&self, ppv: *mut std::ffi::c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        // Safe wrapper for Read
        // ppv is buffer, cb is size
        if ppv.is_null() {
            return E_INVALIDARG; // Invalid arg?
        }

        let buf = unsafe { std::slice::from_raw_parts_mut(ppv as *mut u8, cb as usize) };
        let mut total_read = 0;
        let mut pipe = self.pipe.lock().unwrap();

        while total_read < cb as usize {
            // Check if we have data in current chunk
            if let Some(chunk) = &pipe.current_chunk {
                let remaining = chunk.len() - pipe.cursor;
                if remaining > 0 {
                    let to_copy = std::cmp::min(remaining, (cb as usize) - total_read);
                    buf[total_read..total_read + to_copy]
                        .copy_from_slice(&chunk[pipe.cursor..pipe.cursor + to_copy]);

                    pipe.cursor += to_copy;
                    pipe.total_read += to_copy as u64;
                    total_read += to_copy;
                    continue; // Loop to fill buffer if possible
                } else {
                    // Chunk exhausted
                    pipe.current_chunk = None;
                    pipe.cursor = 0;
                }
            }

            // Need more data
            match pipe.receiver.recv() {
                Ok(Ok(chunk)) => {
                    if chunk.is_empty() {
                        // EOF?
                        break;
                    }
                    pipe.current_chunk = Some(chunk);
                    pipe.cursor = 0;
                }
                Ok(Err(e)) => {
                    tracing::error!("Stream error: {}", e);
                    break;
                }
                Err(_) => {
                    // Disconnected
                    break;
                }
            }
        }

        if !pcbread.is_null() {
            unsafe { *pcbread = total_read as u32 };
        }

        // Send progress update if we have a sender
        if let Some(tx) = &self.progress_tx {
            let percent = if self.file_size > 0 {
                (pipe.total_read as f64 / self.file_size as f64 * 100.0) as u8
            } else {
                0
            };
            // Limit updates to avoid spam? channel is fast enough usually.
            // But we might want to only send if percent changed.
            // For now, simple.
            let _ = tx.send(ProgressUpdate {
                percent,
                message: None, // Or "Transferring..."
            });
        }

        if total_read < cb as usize {
            return S_FALSE;
        }

        S_OK
    }

    fn Write(&self, _pv: *const std::ffi::c_void, _cb: u32, _pcbwritten: *mut u32) -> HRESULT {
        E_NOTIMPL
    }
}

impl IStream_Impl for ArchiveStream {
    fn Seek(
        &self,
        dlibmove: i64,
        dworigin: windows::Win32::System::Com::STREAM_SEEK,
        plibnewposition: *mut u64,
    ) -> Result<()> {
        use windows::Win32::System::Com::{STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET};

        // We can only support basic seeking:
        // SET 0 -> Reset (if supported, or just error if we can't rewind pipe easily without re-opening)
        // CUR 0 -> Get current position
        // END 0 -> Get size (if we know file_size)

        let mut pipe = self.pipe.lock().unwrap();

        let current_pos = pipe.total_read;

        // Calculate new position request
        let new_pos = match dworigin {
            STREAM_SEEK_SET => dlibmove as u64,
            STREAM_SEEK_CUR => (current_pos as i64 + dlibmove) as u64,
            STREAM_SEEK_END => (self.file_size as i64 + dlibmove) as u64,
            _ => return Err(E_INVALIDARG.into()),
        };

        if dworigin == STREAM_SEEK_CUR && dlibmove == 0 {
            // Just asking for position
            if !plibnewposition.is_null() {
                unsafe { *plibnewposition = current_pos };
            }
            return Ok(());
        }

        if dworigin == STREAM_SEEK_END && dlibmove == 0 {
            // Just asking for size (effectively)
            if !plibnewposition.is_null() {
                unsafe { *plibnewposition = self.file_size };
            }
            // But wait, Seek actually moves the pointer.
            // If they seek to end, they want to read from end?
            // If they just want size, they use Stat.
            // But sometimes they seek to end to check size.
            // If we claim to successfully seek to end, next Read returns EOF.
            // We can support this logically.
            pipe.total_read = self.file_size;
            return Ok(());
        }

        // If they try to seek backwards or forwards significantly, we fail because it's a pipe.
        // EXCEPT if they seek to 0 (Rewind).
        if new_pos == 0 {
            // To support rewind, we'd need to restart the thread?
            // Or fail?
            // For now, let's allow it if we are already at 0.
            if current_pos == 0 {
                if !plibnewposition.is_null() {
                    unsafe { *plibnewposition = 0 };
                }
                return Ok(());
            }
            // Real rewind not implemented yet
            return Err(E_NOTIMPL.into());
        }

        // Allow "seeking" to current position (no-op)
        if new_pos == current_pos {
            if !plibnewposition.is_null() {
                unsafe { *plibnewposition = current_pos };
            }
            return Ok(());
        }

        Err(E_NOTIMPL.into())
    }

    fn SetSize(&self, _libnewsize: u64) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn CopyTo(
        &self,
        _pstm: Option<&IStream>,
        _cb: u64,
        _pcbread: *mut u64,
        _pcbwritten: *mut u64,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Commit(&self, _grfcommitflags: &windows::Win32::System::Com::STGC) -> Result<()> {
        Ok(())
    }

    fn Revert(&self) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn LockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: &windows::Win32::System::Com::LOCKTYPE,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn UnlockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Stat(
        &self,
        pstatstg: *mut STATSTG,
        _grfstatflag: &windows::Win32::System::Com::STATFLAG,
    ) -> Result<()> {
        if pstatstg.is_null() {
            return Err(E_NOTIMPL.into()); // Invalid arg
        }

        unsafe {
            (*pstatstg).r#type = STGTY_STREAM.0 as u32;
            (*pstatstg).cbSize = self.file_size;
        }
        Ok(())
    }

    fn Clone(&self) -> Result<IStream> {
        Err(E_NOTIMPL.into())
    }
}
