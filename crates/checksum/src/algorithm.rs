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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_crc32() {
        assert_eq!(Algorithm::from_str("crc32"), Some(Algorithm::Crc32));
        assert_eq!(Algorithm::from_str("CRC32"), Some(Algorithm::Crc32));
    }

    #[test]
    fn from_str_xxhash() {
        assert_eq!(Algorithm::from_str("xxhash"), Some(Algorithm::XxHash));
        assert_eq!(Algorithm::from_str("xxh3"), Some(Algorithm::XxHash));
    }

    #[test]
    fn from_str_sha256() {
        assert_eq!(Algorithm::from_str("sha256"), Some(Algorithm::Sha256));
        assert_eq!(Algorithm::from_str("sha-256"), Some(Algorithm::Sha256));
    }

    #[test]
    fn from_str_invalid() {
        assert_eq!(Algorithm::from_str("md5"), None);
        assert_eq!(Algorithm::from_str(""), None);
    }

    #[test]
    fn output_size_matches_algorithm() {
        assert_eq!(Algorithm::Crc32.output_size(), 4);
        assert_eq!(Algorithm::XxHash.output_size(), 8);
        assert_eq!(Algorithm::Sha256.output_size(), 32);
    }

    #[test]
    fn display_round_trip() {
        for algo in [Algorithm::Crc32, Algorithm::XxHash, Algorithm::Sha256] {
            let s = algo.to_string();
            assert_eq!(Algorithm::from_str(&s), Some(algo));
        }
    }

    #[test]
    fn default_is_crc32() {
        assert_eq!(Algorithm::default(), Algorithm::Crc32);
    }
}
