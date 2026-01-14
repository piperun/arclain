use std::io::Write;
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
    // Keep handle to ensure thread lives until stream matches or is dropped?
    // Actually if we drop handle, thread detaches but continues running until it hits channel close?
    // We'll rename to _handle to suppress warning but keep it.
    _handle: Option<JoinHandle<()>>,
}

#[implement(IStream)]
pub struct ArchiveStream {
    pipe: Mutex<StreamPipe>,
    file_size: u64,
}

impl ArchiveStream {
    /// Create a new ArchiveStream that extracts the given entry on a background thread
    pub fn new(
        backend: Arc<dyn arclain_core::ArchiveBackend>,
        archive_path: std::path::PathBuf,
        entry_name: String,
        password: Option<String>,
        file_size: u64,
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
                _handle: Some(handle),
            }),
            file_size,
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
                    // Return S_FALSE (EOF/Error) implies we stop reading
                    break;
                }
                Err(_) => {
                    // Disconnected (EOF)
                    break;
                }
            }
        }

        if !pcbread.is_null() {
            unsafe { *pcbread = total_read as u32 };
        }

        // Technically if total_read < cb, return S_FALSE? Windows IStream::Read usually returns S_OK if *any* read?
        // Check docs: "Returns S_OK if data was successfully read... S_FALSE if the number of bytes read is less than cb"
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
        _dlibmove: i64,
        _dworigin: windows::Win32::System::Com::STREAM_SEEK,
        _plibnewposition: *mut u64,
    ) -> Result<()> {
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
