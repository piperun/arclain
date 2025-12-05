//! Merkle tree implementation for efficient verification

use crate::algorithm::Algorithm;
use crate::hasher::{FileHashResult, Hash};

/// A Merkle tree for efficient file integrity verification
#[derive(Debug, Clone)]
pub struct MerkleTree {
    /// Root hash of the tree
    root: Hash,
    /// All nodes in the tree, indexed by level then position
    /// Level 0 = leaves (file hashes), Level N = root
    nodes: Vec<Vec<Hash>>,
    /// Original file paths (in order of leaves)
    file_paths: Vec<String>,
    /// Algorithm used
    algorithm: Algorithm,
}

impl MerkleTree {
    /// Build a Merkle tree from file hash results
    pub fn from_file_hashes(files: &[FileHashResult], algorithm: Algorithm) -> Self {
        if files.is_empty() {
            return Self::empty(algorithm);
        }

        // Level 0: leaf nodes (file hashes)
        let leaves: Vec<Hash> = files.iter().map(|f| f.hash.clone()).collect();
        let file_paths: Vec<String> = files.iter().map(|f| f.relative_path.clone()).collect();

        // Sort by path for consistent ordering
        let mut indexed: Vec<_> = leaves
            .iter()
            .cloned()
            .zip(file_paths.iter().cloned())
            .collect();
        indexed.sort_by(|a, b| a.1.cmp(&b.1));

        let leaves: Vec<Hash> = indexed.iter().map(|(h, _)| h.clone()).collect();
        let file_paths: Vec<String> = indexed.iter().map(|(_, p)| p.clone()).collect();

        let mut nodes: Vec<Vec<Hash>> = vec![leaves.clone()];

        // Build tree bottom-up
        let mut current_level = leaves;
        while current_level.len() > 1 {
            let next_level = Self::build_next_level(&current_level, algorithm);
            nodes.push(next_level.clone());
            current_level = next_level;
        }

        let root = current_level
            .into_iter()
            .next()
            .unwrap_or_else(|| Hash::new(algorithm, vec![]));

        Self {
            root,
            nodes,
            file_paths,
            algorithm,
        }
    }

    /// Create an empty Merkle tree
    fn empty(algorithm: Algorithm) -> Self {
        Self {
            root: Hash::new(algorithm, vec![]),
            nodes: vec![],
            file_paths: vec![],
            algorithm,
        }
    }

    /// Build the next level of the tree by combining pairs
    fn build_next_level(current: &[Hash], algorithm: Algorithm) -> Vec<Hash> {
        current
            .chunks(2)
            .map(|chunk| {
                if chunk.len() == 2 {
                    Self::combine_hashes(&chunk[0], &chunk[1], algorithm)
                } else {
                    // Odd node: hash with itself
                    Self::combine_hashes(&chunk[0], &chunk[0], algorithm)
                }
            })
            .collect()
    }

    /// Combine two hashes into a parent hash
    fn combine_hashes(left: &Hash, right: &Hash, algorithm: Algorithm) -> Hash {
        let mut combined = Vec::with_capacity(left.bytes.len() + right.bytes.len());
        combined.extend_from_slice(&left.bytes);
        combined.extend_from_slice(&right.bytes);

        let result_bytes = match algorithm {
            Algorithm::Crc32 => {
                let hash = crc32fast::hash(&combined);
                hash.to_le_bytes().to_vec()
            }
            Algorithm::XxHash => {
                let hash = xxhash_rust::xxh3::xxh3_64(&combined);
                hash.to_le_bytes().to_vec()
            }
            Algorithm::Sha256 => {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(&combined);
                hasher.finalize().to_vec()
            }
        };

        Hash::new(algorithm, result_bytes)
    }

    /// Get the root hash
    pub fn root_hash(&self) -> &Hash {
        &self.root
    }

    /// Get the number of files (leaves)
    pub fn file_count(&self) -> usize {
        self.file_paths.len()
    }

    /// Get the tree depth
    pub fn depth(&self) -> usize {
        self.nodes.len()
    }

    /// Get file paths in order
    pub fn file_paths(&self) -> &[String] {
        &self.file_paths
    }

    /// Get hash for a specific file by index
    pub fn get_file_hash(&self, index: usize) -> Option<&Hash> {
        self.nodes.first()?.get(index)
    }

    /// Get the proof path for a specific file (for partial verification)
    pub fn get_proof(&self, file_index: usize) -> Option<Vec<Hash>> {
        if file_index >= self.file_paths.len() {
            return None;
        }

        let mut proof = Vec::new();
        let mut index = file_index;

        for level in 0..self.nodes.len() - 1 {
            let sibling_index = if index % 2 == 0 { index + 1 } else { index - 1 };

            if let Some(sibling) = self.nodes[level].get(sibling_index) {
                proof.push(sibling.clone());
            } else if let Some(self_hash) = self.nodes[level].get(index) {
                // Odd node case: use self
                proof.push(self_hash.clone());
            }

            index /= 2;
        }

        Some(proof)
    }

    /// Verify that a file hash matches the tree at the given index
    pub fn verify_file(&self, file_index: usize, hash: &Hash) -> bool {
        if let Some(stored) = self.get_file_hash(file_index) {
            stored == hash
        } else {
            false
        }
    }

    /// Serialize to bytes for storage
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut result = Vec::new();

        // Algorithm byte
        result.push(match self.algorithm {
            Algorithm::Crc32 => 0,
            Algorithm::XxHash => 1,
            Algorithm::Sha256 => 2,
        });

        // Number of files (4 bytes, little endian)
        result.extend_from_slice(&(self.file_paths.len() as u32).to_le_bytes());

        // Root hash
        result.extend_from_slice(&(self.root.bytes.len() as u32).to_le_bytes());
        result.extend_from_slice(&self.root.bytes);

        result
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }

        let algorithm = match bytes[0] {
            0 => Algorithm::Crc32,
            1 => Algorithm::XxHash,
            2 => Algorithm::Sha256,
            _ => return None,
        };

        // This is a simplified version that only stores the root
        // Full deserialization would need more data
        let file_count = u32::from_le_bytes(bytes[1..5].try_into().ok()?) as usize;
        let hash_len = u32::from_le_bytes(bytes[5..9].try_into().ok()?) as usize;
        let hash_bytes = bytes[9..9 + hash_len].to_vec();

        Some(Self {
            root: Hash::new(algorithm, hash_bytes),
            nodes: vec![], // Not stored in simple mode
            file_paths: vec!["".to_string(); file_count],
            algorithm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_files() -> Vec<FileHashResult> {
        vec![
            FileHashResult {
                relative_path: "a.txt".to_string(),
                hash: Hash::new(Algorithm::Crc32, vec![1, 2, 3, 4]),
                size: 100,
            },
            FileHashResult {
                relative_path: "b.txt".to_string(),
                hash: Hash::new(Algorithm::Crc32, vec![5, 6, 7, 8]),
                size: 200,
            },
            FileHashResult {
                relative_path: "c.txt".to_string(),
                hash: Hash::new(Algorithm::Crc32, vec![9, 10, 11, 12]),
                size: 300,
            },
        ]
    }

    #[test]
    fn test_merkle_tree_construction() {
        let files = make_test_files();
        let tree = MerkleTree::from_file_hashes(&files, Algorithm::Crc32);

        assert_eq!(tree.file_count(), 3);
        assert!(tree.depth() >= 2); // At least leaves + root
        assert!(!tree.root_hash().bytes.is_empty());
    }

    #[test]
    fn test_merkle_tree_deterministic() {
        let files = make_test_files();
        let tree1 = MerkleTree::from_file_hashes(&files, Algorithm::Crc32);
        let tree2 = MerkleTree::from_file_hashes(&files, Algorithm::Crc32);

        assert_eq!(tree1.root_hash(), tree2.root_hash());
    }

    #[test]
    fn test_merkle_tree_verify_file() {
        let files = make_test_files();
        let tree = MerkleTree::from_file_hashes(&files, Algorithm::Crc32);

        // File at index 0 should verify with its original hash
        let original_hash = &files[0].hash;
        // Need to find correct index after sorting
        let sorted_index = tree.file_paths().iter().position(|p| p == "a.txt").unwrap();
        assert!(tree.verify_file(sorted_index, original_hash));
    }

    #[test]
    fn test_merkle_tree_serialization() {
        let files = make_test_files();
        let tree = MerkleTree::from_file_hashes(&files, Algorithm::Crc32);

        let bytes = tree.to_bytes();
        let restored = MerkleTree::from_bytes(&bytes).unwrap();

        assert_eq!(tree.root_hash(), restored.root_hash());
        assert_eq!(tree.file_count(), restored.file_count());
    }

    #[test]
    fn test_empty_tree() {
        let tree = MerkleTree::from_file_hashes(&[], Algorithm::Crc32);
        assert_eq!(tree.file_count(), 0);
        assert!(tree.root_hash().bytes.is_empty());
    }
}
