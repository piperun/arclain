//! Hash algorithm definitions

use std::fmt;

/// Supported hash algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Algorithm {
    /// CRC32 - Fastest, good for detecting accidental corruption
    #[default]
    Crc32,
    /// XXHash3 - Very fast, modern algorithm with excellent distribution
    XxHash,
    /// SHA-256 - Cryptographically secure, slower
    Sha256,
}

impl Algorithm {
    /// Get the output size in bytes for this algorithm
    pub fn output_size(&self) -> usize {
        match self {
            Algorithm::Crc32 => 4,
            Algorithm::XxHash => 8,
            Algorithm::Sha256 => 32,
        }
    }

    /// Parse algorithm from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "crc32" => Some(Algorithm::Crc32),
            "xxhash" | "xxh3" => Some(Algorithm::XxHash),
            "sha256" | "sha-256" => Some(Algorithm::Sha256),
            _ => None,
        }
    }
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Algorithm::Crc32 => write!(f, "crc32"),
            Algorithm::XxHash => write!(f, "xxhash"),
            Algorithm::Sha256 => write!(f, "sha256"),
        }
    }
}
