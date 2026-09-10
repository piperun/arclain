use crate::ImportError;
#[cfg(not(windows))]
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
/// The archive being read, held so it cannot change underneath the read.
///
/// On Windows the handle is opened with a share mode that refuses every
/// other writer and deleter for as long as it is held, so the bytes read
/// are the bytes on disk; the length and modification time are checked as
/// well. Elsewhere a writer cannot be kept out, so the whole file is hashed
/// when it is opened and again before the result is reported, and a
/// difference is reported as the source having changed. A hash is a full
/// read of the archive, twice, which is why it is not done where the share
/// mode already answers the question.
pub(super) struct Source {
    pub file: File,
    #[cfg(windows)]
    path: PathBuf,
    #[cfg(not(windows))]
    digest: [u8; 32],
    length: u64,
    modified: std::time::SystemTime,
}
impl Source {
    pub fn open(path: &Path, max: u64) -> Result<Self, ImportError> {
        let mut options = OpenOptions::new();
        options.read(true);
        // Holding this handle prevents reopening the path for writes/deletion
        // throughout native RAR listing and decode, including between passes.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(ImportError::Unsupported);
        }
        if metadata.len() > max {
            return Err(ImportError::Limit);
        }
        let source = Self {
            file,
            #[cfg(windows)]
            path: path.to_path_buf(),
            #[cfg(not(windows))]
            digest: [0; 32],
            length: metadata.len(),
            modified: metadata.modified()?,
        };
        #[cfg(not(windows))]
        let source = {
            let mut source = source;
            source.digest = source.hash()?;
            source
        };
        source.check_metadata()?;
        Ok(source)
    }
    fn check_metadata(&self) -> Result<(), ImportError> {
        let metadata = self.file.metadata()?;
        if metadata.len() != self.length || metadata.modified()? != self.modified {
            return Err(ImportError::SourceChanged);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fn hash(&mut self) -> Result<[u8; 32], ImportError> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut digest = Sha256::new();
        let mut buffer = [0; 64 << 10];
        let mut total = 0u64;
        loop {
            let n = self.file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > self.length {
                return Err(ImportError::SourceChanged);
            }
            digest.update(&buffer[..n]);
        }
        if total != self.length {
            return Err(ImportError::SourceChanged);
        }
        self.file.seek(SeekFrom::Start(0))?;
        Ok(digest.finalize().into())
    }
    pub fn magic(&mut self) -> Result<Vec<u8>, ImportError> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = [0; 8];
        let n = self.file.read(&mut bytes)?;
        self.file.seek(SeekFrom::Start(0))?;
        Ok(bytes[..n].to_vec())
    }
    pub fn verify(&mut self) -> Result<(), ImportError> {
        #[cfg(not(windows))]
        {
            self.check_metadata()?;
            if self.hash()? != self.digest {
                return Err(ImportError::SourceChanged);
            }
        }
        self.check_metadata()
    }
    pub fn native_path(&self) -> Result<PathBuf, ImportError> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            Ok(PathBuf::from(format!(
                "/proc/self/fd/{}",
                self.file.as_raw_fd()
            )))
        }
        #[cfg(windows)]
        {
            Ok(self.path.clone())
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            Err(ImportError::Unsupported)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn held_source_rejects_mutation_or_detects_changed_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source");
        std::fs::write(&path, b"original").unwrap();
        let mut source = Source::open(&path, 100).unwrap();
        #[cfg(windows)]
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        #[cfg(target_os = "linux")]
        {
            std::fs::write(&path, b"modified").unwrap();
            assert_eq!(source.verify().unwrap_err(), ImportError::SourceChanged);
        }
        #[cfg(windows)]
        source.verify().unwrap();
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn held_native_path_survives_pathname_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source");
        std::fs::write(&path, b"original").unwrap();
        let mut source = Source::open(&path, 100).unwrap();
        std::fs::rename(&path, dir.path().join("held")).unwrap();
        std::fs::write(&path, b"replaced").unwrap();
        assert_eq!(
            std::fs::read(source.native_path().unwrap()).unwrap(),
            b"original"
        );
        source.verify().unwrap();
    }
}
