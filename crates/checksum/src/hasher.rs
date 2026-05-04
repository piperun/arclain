//! File and folder hashing with parallel processing

use crate::algorithm::Algorithm;
use crate::merkle::MerkleTree;
use anyhow::{Context, Result};
use memmap2::Mmap;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use walkdir::WalkDir;

/// A computed hash value
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hash {
    pub algorithm: Algorithm,
    pub bytes: Vec<u8>,
}

impl Hash {
    pub fn new(algorithm: Algorithm, bytes: Vec<u8>) -> Self {
        Self { algorithm, bytes }
    }

    /// Convert to hex string for display
    pub fn to_hex(&self) -> String {
        self.bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Create from raw bytes
    pub fn from_bytes(algorithm: Algorithm, bytes: &[u8]) -> Self {
        Self {
            algorithm,
            bytes: bytes.to_vec(),
        }
    }
}

/// Threshold for using memory-mapped I/O (10 MB)
const MMAP_THRESHOLD: u64 = 10 * 1024 * 1024;

/// Hash a single file.
///
/// **Synchronous: do not call from inside an async task** — the
/// `std::fs` operations and the actual hashing both block the calling
/// thread. From a tokio context use
/// `tokio::task::spawn_blocking(|| hash_file(...))` (audit finding M2).
pub fn hash_file(path: &Path, algorithm: Algorithm) -> Result<Hash> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read metadata for {}", path.display()))?;

    let file_size = metadata.len();

    if file_size > MMAP_THRESHOLD {
        hash_file_mmap(path, algorithm)
    } else {
        hash_file_read(path, algorithm)
    }
}

/// Hash a file by reading it into memory (for small files)
fn hash_file_read(path: &Path, algorithm: Algorithm) -> Result<Hash> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let bytes = compute_hash(&buffer, algorithm);
    Ok(Hash::new(algorithm, bytes))
}

/// Hash a file using memory-mapped I/O (for large files)
fn hash_file_mmap(path: &Path, algorithm: Algorithm) -> Result<Hash> {
    let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;

    // SAFETY: We're only reading and the file won't be modified during hashing
    let mmap = unsafe { Mmap::map(&file) }
        .with_context(|| format!("Failed to mmap {}", path.display()))?;

    let bytes = compute_hash(&mmap, algorithm);
    Ok(Hash::new(algorithm, bytes))
}

/// Compute hash of a byte slice using the specified algorithm
fn compute_hash(data: &[u8], algorithm: Algorithm) -> Vec<u8> {
    match algorithm {
        Algorithm::Crc32 => {
            let hash = crc32fast::hash(data);
            hash.to_le_bytes().to_vec()
        }
        Algorithm::XxHash => {
            let hash = xxhash_rust::xxh3::xxh3_64(data);
            hash.to_le_bytes().to_vec()
        }
        Algorithm::Sha256 => {
            let mut hasher = Sha256::new();
            hasher.update(data);
            hasher.finalize().to_vec()
        }
    }
}

/// Result of hashing a single file within a folder
#[derive(Debug, Clone)]
pub struct FileHashResult {
    /// Relative path from the root folder
    pub relative_path: String,
    /// Computed hash
    pub hash: Hash,
    /// File size in bytes
    pub size: u64,
}

/// Hash all files in a folder in parallel
pub fn hash_folder_parallel(
    root: &Path,
    algorithm: Algorithm,
    max_threads: Option<usize>,
) -> Result<(Vec<FileHashResult>, MerkleTree)> {
    // Configure thread pool if specified
    if let Some(threads) = max_threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .ok(); // Ignore error if already initialized
    }

    // Collect all file paths first
    let files: Vec<_> = WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    tracing::info!("Hashing {} files in parallel", files.len());

    // Hash files in parallel
    let results: Vec<FileHashResult> = files
        .par_iter()
        .filter_map(|entry| {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");

            match hash_file(path, algorithm) {
                Ok(hash) => {
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    Some(FileHashResult {
                        relative_path: relative,
                        hash,
                        size,
                    })
                }
                Err(e) => {
                    tracing::warn!("Failed to hash {}: {}", path.display(), e);
                    None
                }
            }
        })
        .collect();

    // Build Merkle tree from results
    let merkle = MerkleTree::from_file_hashes(&results, algorithm);

    Ok((results, merkle))
}

/// Hash data from a stream (for in-archive files)
pub fn hash_stream<R: Read>(mut reader: R, algorithm: Algorithm) -> Result<Hash> {
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;
    let bytes = compute_hash(&buffer, algorithm);
    Ok(Hash::new(algorithm, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    // === Hash struct tests ===

    #[test]
    fn hash_to_hex_known_bytes() {
        let hash = Hash::from_bytes(Algorithm::Crc32, &[0xAB, 0xCD, 0xEF, 0x01]);
        assert_eq!(hash.to_hex(), "abcdef01");
    }

    #[test]
    fn hash_to_hex_empty() {
        let hash = Hash::from_bytes(Algorithm::Crc32, &[]);
        assert_eq!(hash.to_hex(), "");
    }

    #[test]
    fn hash_from_bytes_preserves_algorithm() {
        let hash = Hash::from_bytes(Algorithm::Sha256, &[0x00, 0xFF]);
        assert_eq!(hash.algorithm, Algorithm::Sha256);
        assert_eq!(hash.bytes, vec![0x00, 0xFF]);
    }

    #[test]
    fn hash_new_equals_from_bytes() {
        let a = Hash::new(Algorithm::XxHash, vec![1, 2, 3]);
        let b = Hash::from_bytes(Algorithm::XxHash, &[1, 2, 3]);
        assert_eq!(a, b);
    }

    // === File-based tests ===

    #[test]
    fn test_hash_file_crc32() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"Hello, World!").unwrap();

        let hash = hash_file(&file_path, Algorithm::Crc32).unwrap();
        assert_eq!(hash.algorithm, Algorithm::Crc32);
        assert_eq!(hash.bytes.len(), 4);
    }

    #[test]
    fn test_hash_file_xxhash() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"Hello, World!").unwrap();

        let hash = hash_file(&file_path, Algorithm::XxHash).unwrap();
        assert_eq!(hash.algorithm, Algorithm::XxHash);
        assert_eq!(hash.bytes.len(), 8);
    }

    #[test]
    fn test_hash_file_sha256() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"Hello, World!").unwrap();

        let hash = hash_file(&file_path, Algorithm::Sha256).unwrap();
        assert_eq!(hash.algorithm, Algorithm::Sha256);
        assert_eq!(hash.bytes.len(), 32);
    }

    #[test]
    fn test_deterministic_hashing() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"Consistent content").unwrap();

        let hash1 = hash_file(&file_path, Algorithm::Crc32).unwrap();
        let hash2 = hash_file(&file_path, Algorithm::Crc32).unwrap();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_folder_parallel() {
        let dir = TempDir::new().unwrap();

        // Create some test files
        for i in 0..5 {
            let file_path = dir.path().join(format!("file{}.txt", i));
            let mut file = File::create(&file_path).unwrap();
            writeln!(file, "Content {}", i).unwrap();
        }

        let (results, merkle) = hash_folder_parallel(dir.path(), Algorithm::Crc32, None).unwrap();
        assert_eq!(results.len(), 5);
        assert!(!merkle.root_hash().bytes.is_empty());
    }
}
