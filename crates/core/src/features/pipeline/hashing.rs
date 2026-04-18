//! Hashing helpers for pipeline dedup.
//!
//! Input archives are fingerprinted with Blake3 at pipeline start. The hash
//! is combined with `Pipeline::config_hash()` to key `pipeline_runs` rows so
//! a re-run of the exact same work can be detected and skipped.
//!
//! Blake3 is fast (~2 GB/s single-threaded on modern hardware); a 10 GB
//! archive hashes in ~5 s, which fits inside a batch that was already going
//! to take minutes to extract + re-pack.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

const HASH_BUF_SIZE: usize = 64 * 1024;

/// Streaming Blake3 hash of the file at `path`.
/// Returns `(hex_digest, file_size_bytes)`.
pub fn hash_file_blake3(path: &Path) -> Result<(String, u64)> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("opening {:?} for hashing", path))?;
    let size = file
        .metadata()
        .with_context(|| format!("stat {:?}", path))?
        .len();

    let mut reader = std::io::BufReader::with_capacity(HASH_BUF_SIZE, file);
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; HASH_BUF_SIZE];
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("reading {:?} for hashing", path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok((hasher.finalize().to_hex().to_string(), size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_for_same_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.bin");
        std::fs::write(&p, b"hello world").unwrap();
        let (h1, s1) = hash_file_blake3(&p).unwrap();
        let (h2, s2) = hash_file_blake3(&p).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(s1, s2);
        assert_eq!(s1, 11);
        assert_eq!(h1.len(), 64); // 32-byte hash, 64 hex chars
    }

    #[test]
    fn hash_differs_for_different_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.bin");
        let b = tmp.path().join("b.bin");
        std::fs::write(&a, b"aaaa").unwrap();
        std::fs::write(&b, b"bbbb").unwrap();
        assert_ne!(hash_file_blake3(&a).unwrap().0, hash_file_blake3(&b).unwrap().0);
    }

    #[test]
    fn hashes_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("empty.bin");
        std::fs::write(&p, b"").unwrap();
        let (h, s) = hash_file_blake3(&p).unwrap();
        assert_eq!(s, 0);
        // Blake3 of empty input is a known constant
        assert_eq!(
            h,
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }
}
