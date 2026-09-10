use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::PathBuf;

pub(crate) const MAX_FRAME: usize = 64 << 10;
pub(crate) const READY: u8 = 1;
pub(crate) const OPEN: u8 = 2;
pub(crate) const ARCHIVE: u8 = 3;
pub(crate) const BLOCK: u8 = 4;
pub(crate) const ENTRY: u8 = 5;
pub(crate) const INVENTORY_END: u8 = 6;
pub(crate) const DECODE: u8 = 7;
pub(crate) const BEGIN: u8 = 8;
pub(crate) const DATA: u8 = 9;
pub(crate) const END: u8 = 10;
pub(crate) const COMPLETE: u8 = 11;
pub(crate) const FAILURE: u8 = 12;
/// Written by the worker when nothing else was written for five seconds, so
/// a long silent phase is not read as a stalled worker. Carries nothing.
pub(crate) const HEARTBEAT: u8 = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ImportError {
    #[error("archive worker is unavailable")]
    Unavailable,
    #[error("archive worker could not establish its resource limit")]
    ResourceLimit,
    #[error("archive worker failed or exceeded its allocation limit")]
    WorkerFailed,
    #[error("archive metadata cannot be faithfully imported")]
    Unsupported,
    #[error("archive is encrypted")]
    Encrypted,
    #[error("archive input changed during decoding")]
    SourceChanged,
    #[error("archive data or worker protocol is invalid")]
    Corrupt,
    #[error("archive exceeds its configured limit")]
    Limit,
    #[error("archive import was cancelled")]
    Cancelled,
    #[error("archive worker stopped making progress")]
    TimedOut,
    #[error("archive input or worker pipe could not be read")]
    Io,
}
impl From<io::Error> for ImportError {
    fn from(_: io::Error) -> Self {
        Self::Io
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Request {
    pub source: PathBuf,
    pub limits: WireLimits,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct WireLimits {
    pub entries: u64,
    pub total: u64,
    pub ratio: u32,
    pub archive: u64,
    pub index: u64,
}
impl From<unboxer::Limits> for WireLimits {
    fn from(l: unboxer::Limits) -> Self {
        Self {
            entries: l.max_entries,
            total: l.max_total_bytes,
            ratio: l.max_compression_ratio,
            archive: l.max_archive_bytes,
            index: l.max_index_bytes,
        }
    }
}
#[cfg(feature = "worker")]
impl WireLimits {
    pub fn unpack(&self) -> unboxer::Limits {
        let mut l = unboxer::Limits::new(self.entries, self.total, self.ratio, self.archive);
        l.max_index_bytes = self.index;
        l
    }
}

pub(crate) fn frame(writer: &mut dyn Write, tag: u8, bytes: &[u8]) -> Result<(), ImportError> {
    if bytes.len() > MAX_FRAME {
        return Err(ImportError::Limit);
    }
    // One write per frame: the worker's stdout is shared with its heartbeat
    // thread, and two frames must never interleave.
    let mut out = Vec::with_capacity(5 + bytes.len());
    out.push(tag);
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    writer.write_all(&out)?;
    writer.flush()?;
    Ok(())
}
pub(crate) fn json<T: Serialize>(
    writer: &mut dyn Write,
    tag: u8,
    value: &T,
) -> Result<(), ImportError> {
    frame(
        writer,
        tag,
        &serde_json::to_vec(value).map_err(|_| ImportError::Corrupt)?,
    )
}
pub(crate) fn read_frame(reader: &mut dyn Read) -> Result<(u8, Vec<u8>), ImportError> {
    let mut header = [0; 5];
    reader.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header[1..].try_into().unwrap()) as usize;
    if length > MAX_FRAME {
        return Err(ImportError::Limit);
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok((header[0], bytes))
}
pub(crate) fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ImportError> {
    serde_json::from_slice(bytes).map_err(|_| ImportError::Corrupt)
}
pub(crate) fn index(bytes: &[u8]) -> Result<usize, ImportError> {
    let bytes: [u8; 8] = bytes.try_into().map_err(|_| ImportError::Corrupt)?;
    usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| ImportError::Limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_and_truncated_frames_are_refused_before_payload_allocation() {
        let mut header = vec![DATA];
        header.extend_from_slice(&((MAX_FRAME + 1) as u32).to_le_bytes());
        assert_eq!(
            read_frame(&mut header.as_slice()).unwrap_err(),
            ImportError::Limit
        );
        for bytes in [&[DATA][..], &[DATA, 2, 0, 0, 0, 1][..]] {
            assert!(read_frame(&mut &*bytes).is_err());
        }
    }

    #[test]
    fn a_frame_is_one_write() {
        struct Counting {
            bytes: Vec<u8>,
            writes: usize,
        }
        impl Write for Counting {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.writes += 1;
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut sink = Counting {
            bytes: vec![],
            writes: 0,
        };
        frame(&mut sink, DATA, b"abc").unwrap();
        assert_eq!(
            sink.writes, 1,
            "a shared writer must never see half a frame"
        );
        assert_eq!(sink.bytes, [DATA, 3, 0, 0, 0, b'a', b'b', b'c']);

        let oversized = vec![0; MAX_FRAME + 1];
        frame(&mut sink, DATA, &oversized[..MAX_FRAME]).unwrap();
        assert_eq!(sink.writes, 2, "the largest frame is one write too");
        assert_eq!(sink.bytes.len(), 5 + 3 + 5 + MAX_FRAME);

        assert_eq!(
            frame(&mut sink, DATA, &oversized).unwrap_err(),
            ImportError::Limit
        );
        assert_eq!(sink.writes, 2, "an oversized frame writes nothing at all");
    }
}
