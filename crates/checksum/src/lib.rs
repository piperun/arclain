//! File integrity verification with Merkle trees and parallel hashing
//!
//! This crate provides:
//! - Multiple hash algorithms (CRC32, XXHash, SHA-256)
//! - Parallel folder hashing using rayon
//! - Merkle tree construction for efficient verification
//!
//! Database storage is handled by the `arclain_db` crate.

pub mod algorithm;
pub mod hasher;
pub mod merkle;

pub use algorithm::Algorithm;
pub use hasher::{hash_file, hash_folder_parallel, hash_stream, FileHashResult, Hash};
pub use merkle::MerkleTree;
