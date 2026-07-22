//! Merkle tree implementation for efficient verification

use crate::algorithm::Algorithm;
use crate::hasher::{FileHashResult, Hash};

const MERKLE_ENCODING_VERSION: u8 = 2;
const LEAF_DOMAIN: &[u8] = b"arclain-merkle-leaf";
const NODE_DOMAIN: &[u8] = b"arclain-merkle-node";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PathNamespace {
    Empty,
    Current,
    Relative,
    Rooted,
    Unc,
    DriveRelative,
    DriveRooted,
}

impl PathNamespace {
    fn tag(self) -> u8 {
        match self {
            Self::Empty => 0,
            Self::Current => 1,
            Self::Relative => 2,
            Self::Rooted => 3,
            Self::Unc => 4,
            Self::DriveRelative => 5,
            Self::DriveRooted => 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedPath {
    namespace: PathNamespace,
    value: String,
}

fn algorithm_tag(algorithm: Algorithm) -> u8 {
    match algorithm {
        Algorithm::Crc32 => 0,
        Algorithm::XxHash => 1,
        Algorithm::Sha256 => 2,
    }
}

fn append_length_prefixed(encoded: &mut Vec<u8>, field: &[u8]) {
    encoded.extend_from_slice(&(field.len() as u64).to_le_bytes());
    encoded.extend_from_slice(field);
}

fn hash_encoded(encoded: &[u8], algorithm: Algorithm) -> Hash {
    let bytes = match algorithm {
        Algorithm::Crc32 => crc32fast::hash(encoded).to_le_bytes().to_vec(),
        Algorithm::XxHash => xxhash_rust::xxh3::xxh3_64(encoded).to_le_bytes().to_vec(),
        Algorithm::Sha256 => {
            use sha2::{Digest, Sha256};
            Sha256::digest(encoded).to_vec()
        }
    };

    Hash::new(algorithm, bytes)
}

fn normalize_components(path: &str) -> String {
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => match components.last() {
                Some(previous) if *previous != ".." => {
                    components.pop();
                }
                _ => components.push(component),
            },
            _ => components.push(component),
        }
    }
    components.join("/")
}

fn normalize_unc_path(path: &str) -> String {
    let components: Vec<&str> = path
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect();
    let authority_length = components.len().min(2);
    let authority = components[..authority_length].join("/");
    let tail = normalize_components(&components[authority_length..].join("/"));

    match (authority.is_empty(), tail.is_empty()) {
        (true, _) => "//".to_string(),
        (false, true) => format!("//{authority}"),
        (false, false) => format!("//{authority}/{tail}"),
    }
}

fn has_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn normalize_relative_path(path: &str) -> NormalizedPath {
    if path.is_empty() {
        return NormalizedPath {
            namespace: PathNamespace::Empty,
            value: String::new(),
        };
    }

    let portable = path.replace('\\', "/");
    if has_drive_prefix(&portable) {
        let drive = &portable[..2];
        let remainder = &portable[2..];
        let rooted = remainder.starts_with('/');
        let normalized = normalize_components(remainder.trim_start_matches('/'));
        let value = if rooted {
            if normalized.is_empty() {
                format!("{drive}/")
            } else {
                format!("{drive}/{normalized}")
            }
        } else {
            format!("{drive}{normalized}")
        };

        return NormalizedPath {
            namespace: if rooted {
                PathNamespace::DriveRooted
            } else {
                PathNamespace::DriveRelative
            },
            value,
        };
    }

    let (namespace, remainder) = if portable.starts_with("//") {
        (PathNamespace::Unc, portable.trim_start_matches('/'))
    } else if portable.starts_with('/') {
        (PathNamespace::Rooted, portable.trim_start_matches('/'))
    } else {
        (PathNamespace::Relative, portable.as_str())
    };
    let normalized = if namespace == PathNamespace::Unc {
        normalize_unc_path(remainder)
    } else {
        normalize_components(remainder)
    };

    match namespace {
        PathNamespace::Relative if normalized.is_empty() => NormalizedPath {
            namespace: PathNamespace::Current,
            value: ".".to_string(),
        },
        PathNamespace::Rooted => NormalizedPath {
            namespace,
            value: if normalized.is_empty() {
                "/".to_string()
            } else {
                format!("/{normalized}")
            },
        },
        PathNamespace::Unc => NormalizedPath {
            namespace,
            value: normalized,
        },
        _ => NormalizedPath {
            namespace,
            value: normalized,
        },
    }
}

fn hash_leaf(
    normalized_path: &NormalizedPath,
    file: &FileHashResult,
    algorithm: Algorithm,
) -> Hash {
    let mut encoded = Vec::new();
    append_length_prefixed(&mut encoded, LEAF_DOMAIN);
    encoded.push(MERKLE_ENCODING_VERSION);
    encoded.push(algorithm_tag(algorithm));
    encoded.push(normalized_path.namespace.tag());
    append_length_prefixed(&mut encoded, normalized_path.value.as_bytes());
    encoded.extend_from_slice(&file.size.to_le_bytes());
    encoded.push(algorithm_tag(file.hash.algorithm));
    append_length_prefixed(&mut encoded, &file.hash.bytes);
    hash_encoded(&encoded, algorithm)
}

/// A Merkle tree for efficient file integrity verification
#[derive(Debug, Clone)]
pub struct MerkleTree {
    /// Root hash of the tree
    root: Hash,
    /// Identity commitment nodes, indexed by level then position.
    /// Level 0 binds normalized path, size, and content hash; level N is root.
    nodes: Vec<Vec<Hash>>,
    /// Original content hashes in the same sorted order as `file_paths`.
    content_hashes: Vec<Hash>,
    /// Normalized relative file paths (in order of leaves)
    file_paths: Vec<String>,
    /// Number of files committed by the root, including root-only trees.
    file_count: usize,
    /// Algorithm used
    algorithm: Algorithm,
}

impl MerkleTree {
    /// Build a Merkle tree from file hash results.
    ///
    /// Pre-fix (audit P12) each leaf was cloned five times: once into
    /// `leaves`, once into the sort tuple, once back into a sorted
    /// `leaves`, once into the seed of `nodes`, and again into a
    /// parallel `current_level` for the build loop. For a 10k-file
    /// archive that's 50k+ extra `Hash` clones plus 50k+ `String`
    /// clones for paths.
    ///
    /// Now we normalize and sort an index permutation, materialise identity
    /// leaves and public content hashes exactly once each, and borrow each
    /// level back out of `nodes` instead of cloning it into a parallel
    /// `current_level`.
    pub fn from_file_hashes(files: &[FileHashResult], algorithm: Algorithm) -> Self {
        if files.is_empty() {
            return Self::empty(algorithm);
        }

        // Normalize before sorting so equivalent path spellings produce the
        // same tree on every platform. Identity fields break ties so even
        // duplicate normalized paths remain input-order independent.
        let mut entries: Vec<(usize, NormalizedPath)> = files
            .iter()
            .enumerate()
            .map(|(index, file)| (index, normalize_relative_path(&file.relative_path)))
            .collect();
        entries.sort_by(|(a, a_path), (b, b_path)| {
            a_path
                .value
                .cmp(&b_path.value)
                .then_with(|| a_path.namespace.cmp(&b_path.namespace))
                .then_with(|| files[*a].size.cmp(&files[*b].size))
                .then_with(|| {
                    algorithm_tag(files[*a].hash.algorithm)
                        .cmp(&algorithm_tag(files[*b].hash.algorithm))
                })
                .then_with(|| files[*a].hash.bytes.cmp(&files[*b].hash.bytes))
        });

        let file_paths: Vec<String> = entries.iter().map(|(_, path)| path.value.clone()).collect();
        let content_hashes: Vec<Hash> = entries
            .iter()
            .map(|(index, _)| files[*index].hash.clone())
            .collect();
        let leaves: Vec<Hash> = entries
            .iter()
            .map(|(index, path)| hash_leaf(path, &files[*index], algorithm))
            .collect();

        // `nodes[0]` owns the leaves; subsequent levels are appended.
        // `current_idx` indexes the level we're folding, so the loop
        // never needs a parallel `current_level` clone.
        let mut nodes: Vec<Vec<Hash>> = vec![leaves];
        let mut current_idx = 0;
        while nodes[current_idx].len() > 1 {
            let next = Self::build_next_level(&nodes[current_idx], algorithm);
            nodes.push(next);
            current_idx += 1;
        }

        let root = nodes
            .last()
            .and_then(|level| level.first())
            .cloned()
            .unwrap_or_else(|| Hash::new(algorithm, vec![]));

        Self {
            root,
            nodes,
            content_hashes,
            file_count: file_paths.len(),
            file_paths,
            algorithm,
        }
    }

    /// Create an empty Merkle tree
    fn empty(algorithm: Algorithm) -> Self {
        Self {
            root: Hash::new(algorithm, vec![]),
            nodes: vec![],
            content_hashes: vec![],
            file_paths: vec![],
            file_count: 0,
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
        let mut encoded = Vec::new();
        append_length_prefixed(&mut encoded, NODE_DOMAIN);
        encoded.push(MERKLE_ENCODING_VERSION);
        encoded.push(algorithm_tag(algorithm));
        encoded.push(algorithm_tag(left.algorithm));
        append_length_prefixed(&mut encoded, &left.bytes);
        encoded.push(algorithm_tag(right.algorithm));
        append_length_prefixed(&mut encoded, &right.bytes);
        hash_encoded(&encoded, algorithm)
    }

    /// Get the root hash
    pub fn root_hash(&self) -> &Hash {
        &self.root
    }

    /// Get the number of files (leaves)
    pub fn file_count(&self) -> usize {
        self.file_count
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
        self.content_hashes.get(index)
    }

    /// Get sibling identity-commitment hashes for a file.
    ///
    /// The proof starts from the v2 leaf commitment to normalized path, size,
    /// and content hash. It does not start from the public content hash
    /// returned by [`Self::get_file_hash`]. Callers must retain that file
    /// identity to reconstruct the leaf. Root-only deserialization cannot
    /// reconstruct proof nodes and therefore returns `None`.
    pub fn get_proof(&self, file_index: usize) -> Option<Vec<Hash>> {
        if file_index >= self.content_hashes.len() || self.nodes.is_empty() {
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
        result.extend_from_slice(&(self.file_count as u32).to_le_bytes());

        // Root hash
        result.extend_from_slice(&(self.root.bytes.len() as u32).to_le_bytes());
        result.extend_from_slice(&self.root.bytes);

        result
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        const HEADER_LENGTH: usize = 9;
        let header = bytes.get(..HEADER_LENGTH)?;

        let algorithm = match header[0] {
            0 => Algorithm::Crc32,
            1 => Algorithm::XxHash,
            2 => Algorithm::Sha256,
            _ => return None,
        };

        // This is a simplified version that only stores the root
        // Full deserialization would need more data
        let file_count = u32::from_le_bytes(header[1..5].try_into().ok()?) as usize;
        let hash_len = u32::from_le_bytes(header[5..9].try_into().ok()?) as usize;
        let expected_hash_len = if file_count == 0 {
            0
        } else {
            algorithm.output_size()
        };
        if hash_len != expected_hash_len {
            return None;
        }
        let hash_end = HEADER_LENGTH.checked_add(hash_len)?;
        if hash_end != bytes.len() {
            return None;
        }
        let hash_bytes = bytes.get(HEADER_LENGTH..hash_end)?.to_vec();

        Some(Self {
            root: Hash::new(algorithm, hash_bytes),
            nodes: vec![],          // Not stored in simple mode
            content_hashes: vec![], // Not stored in simple mode
            file_paths: vec![],     // Not stored in simple mode
            file_count,
            algorithm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn append_test_field(encoded: &mut Vec<u8>, field: &[u8]) {
        encoded.extend_from_slice(&(field.len() as u64).to_le_bytes());
        encoded.extend_from_slice(field);
    }

    fn expected_sha256_leaf(path: &str, size: u64, content_hash: &Hash) -> Hash {
        let mut encoded = Vec::new();
        append_test_field(&mut encoded, b"arclain-merkle-leaf");
        encoded.push(2);
        encoded.push(2);
        encoded.push(2);
        append_test_field(&mut encoded, path.as_bytes());
        encoded.extend_from_slice(&size.to_le_bytes());
        encoded.push(2);
        append_test_field(&mut encoded, &content_hash.bytes);

        Hash::new(Algorithm::Sha256, Sha256::digest(&encoded).to_vec())
    }

    fn expected_sha256_node(left: &Hash, right: &Hash) -> Hash {
        let mut encoded = Vec::new();
        append_test_field(&mut encoded, b"arclain-merkle-node");
        encoded.push(2);
        encoded.push(2);
        encoded.push(2);
        append_test_field(&mut encoded, &left.bytes);
        encoded.push(2);
        append_test_field(&mut encoded, &right.bytes);

        Hash::new(Algorithm::Sha256, Sha256::digest(&encoded).to_vec())
    }

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
    fn merkle_root_commits_to_relative_path() {
        let original = vec![FileHashResult {
            relative_path: "before.txt".to_string(),
            hash: Hash::new(Algorithm::Sha256, vec![7; 32]),
            size: 42,
        }];
        let mut renamed = original.clone();
        renamed[0].relative_path = "after.txt".to_string();

        let original_tree = MerkleTree::from_file_hashes(&original, Algorithm::Sha256);
        let renamed_tree = MerkleTree::from_file_hashes(&renamed, Algorithm::Sha256);

        assert_ne!(original_tree.root_hash(), renamed_tree.root_hash());
    }

    #[test]
    fn merkle_root_commits_to_file_size() {
        let original = vec![FileHashResult {
            relative_path: "file.txt".to_string(),
            hash: Hash::new(Algorithm::Sha256, vec![7; 32]),
            size: 42,
        }];
        let mut resized = original.clone();
        resized[0].size = 43;

        let original_tree = MerkleTree::from_file_hashes(&original, Algorithm::Sha256);
        let resized_tree = MerkleTree::from_file_hashes(&resized, Algorithm::Sha256);

        assert_ne!(original_tree.root_hash(), resized_tree.root_hash());
    }

    #[test]
    fn sha256_leaf_encoding_is_versioned_and_domain_separated() {
        let content_hash = Hash::new(Algorithm::Sha256, vec![7; 32]);
        let files = vec![FileHashResult {
            relative_path: "file.txt".to_string(),
            hash: content_hash.clone(),
            size: 42,
        }];

        let tree = MerkleTree::from_file_hashes(&files, Algorithm::Sha256);

        assert_eq!(
            tree.root_hash(),
            &expected_sha256_leaf("file.txt", 42, &content_hash)
        );
    }

    #[test]
    fn sha256_node_encoding_is_versioned_and_domain_separated() {
        let files = vec![
            FileHashResult {
                relative_path: "a.txt".to_string(),
                hash: Hash::new(Algorithm::Sha256, vec![1; 32]),
                size: 1,
            },
            FileHashResult {
                relative_path: "b.txt".to_string(),
                hash: Hash::new(Algorithm::Sha256, vec![2; 32]),
                size: 2,
            },
        ];
        let left = expected_sha256_leaf("a.txt", 1, &files[0].hash);
        let right = expected_sha256_leaf("b.txt", 2, &files[1].hash);

        let tree = MerkleTree::from_file_hashes(&files, Algorithm::Sha256);

        assert_eq!(tree.root_hash(), &expected_sha256_node(&left, &right));
    }

    #[test]
    fn merkle_normalizes_relative_path_components() {
        let canonical = vec![FileHashResult {
            relative_path: "folder/file.txt".to_string(),
            hash: Hash::new(Algorithm::Sha256, vec![7; 32]),
            size: 42,
        }];
        let mut alternate = canonical.clone();
        alternate[0].relative_path = "folder\\sub/.././file.txt".to_string();

        let canonical_tree = MerkleTree::from_file_hashes(&canonical, Algorithm::Sha256);
        let alternate_tree = MerkleTree::from_file_hashes(&alternate, Algorithm::Sha256);

        assert_eq!(canonical_tree.root_hash(), alternate_tree.root_hash());
        assert_eq!(alternate_tree.file_paths(), &["folder/file.txt"]);
    }

    #[test]
    fn merkle_sorts_by_normalized_relative_path() {
        let files = vec![
            FileHashResult {
                relative_path: "z/../b.txt".to_string(),
                hash: Hash::new(Algorithm::Crc32, vec![2; 4]),
                size: 2,
            },
            FileHashResult {
                relative_path: "a.txt".to_string(),
                hash: Hash::new(Algorithm::Crc32, vec![1; 4]),
                size: 1,
            },
        ];

        let tree = MerkleTree::from_file_hashes(&files, Algorithm::Crc32);

        assert_eq!(tree.file_paths(), &["a.txt", "b.txt"]);
        assert_eq!(tree.get_file_hash(0), Some(&files[1].hash));
        assert_eq!(tree.get_file_hash(1), Some(&files[0].hash));
    }

    fn tree_for_path(path: &str) -> MerkleTree {
        MerkleTree::from_file_hashes(
            &[FileHashResult {
                relative_path: path.to_string(),
                hash: Hash::new(Algorithm::Sha256, vec![7; 32]),
                size: 42,
            }],
            Algorithm::Sha256,
        )
    }

    fn serialized_root(algorithm: u8, file_count: u32, root: &[u8]) -> Vec<u8> {
        let mut bytes = vec![algorithm];
        bytes.extend_from_slice(&file_count.to_le_bytes());
        bytes.extend_from_slice(&(root.len() as u32).to_le_bytes());
        bytes.extend_from_slice(root);
        bytes
    }

    #[test]
    fn merkle_path_namespace_distinguishes_rooted_paths_from_relative_paths() {
        let relative = tree_for_path("folder/file.txt");
        let slash_rooted = tree_for_path("/folder/file.txt");
        let backslash_rooted = tree_for_path(r"\folder\file.txt");

        assert_ne!(relative.root_hash(), slash_rooted.root_hash());
        assert_ne!(relative.root_hash(), backslash_rooted.root_hash());
        assert_eq!(slash_rooted.root_hash(), backslash_rooted.root_hash());
    }

    #[test]
    fn merkle_path_namespace_distinguishes_unc_paths_from_relative_paths() {
        let relative = tree_for_path("server/share/file.txt");
        let slash_unc = tree_for_path("//server/share/file.txt");
        let backslash_unc = tree_for_path(r"\\server\share\file.txt");

        assert_ne!(relative.root_hash(), slash_unc.root_hash());
        assert_eq!(slash_unc.root_hash(), backslash_unc.root_hash());
    }

    #[test]
    fn merkle_unc_namespace_preserves_server_and_share_authority() {
        let crossing_share_root = tree_for_path("//server/share/../file.txt");
        let nested_crossing = tree_for_path("//server/share/folder/../../file.txt");
        let different_share = tree_for_path("//server/file.txt");

        assert_ne!(crossing_share_root.root_hash(), different_share.root_hash());
        assert_eq!(crossing_share_root.root_hash(), nested_crossing.root_hash());
        assert_eq!(
            crossing_share_root.file_paths(),
            &["//server/share/../file.txt"]
        );
    }

    #[test]
    fn merkle_unc_share_crossing_normalizes_slash_styles_equivalently() {
        let slash_unc = tree_for_path("//server/share/../file.txt");
        let backslash_unc = tree_for_path(r"\\server\share\..\file.txt");

        assert_eq!(slash_unc.root_hash(), backslash_unc.root_hash());
        assert_eq!(slash_unc.file_paths(), backslash_unc.file_paths());
    }

    #[test]
    fn merkle_path_namespace_distinguishes_empty_dot_and_root_only_paths() {
        let empty = tree_for_path("");
        let dot = tree_for_path("./.");
        let slash_root = tree_for_path("/");
        let backslash_root = tree_for_path(r"\");

        assert_ne!(empty.root_hash(), dot.root_hash());
        assert_ne!(empty.root_hash(), slash_root.root_hash());
        assert_ne!(dot.root_hash(), slash_root.root_hash());
        assert_eq!(slash_root.root_hash(), backslash_root.root_hash());
    }

    #[test]
    fn merkle_path_namespace_retains_leading_parent_components() {
        let contained = tree_for_path("file.txt");
        let escaping = tree_for_path("../file.txt");
        let equivalent_escape = tree_for_path("folder/../../file.txt");

        assert_ne!(contained.root_hash(), escaping.root_hash());
        assert_eq!(escaping.root_hash(), equivalent_escape.root_hash());
        assert_eq!(escaping.file_paths(), &["../file.txt"]);
    }

    #[test]
    fn merkle_path_namespace_distinguishes_drive_forms() {
        let drive_rooted = tree_for_path(r"C:\folder\file.txt");
        let drive_rooted_slashes = tree_for_path("C:/folder/file.txt");
        let drive_relative = tree_for_path("C:folder/file.txt");
        let drive_looking_relative = tree_for_path("prefix/../C:/folder/file.txt");
        let drive_relative_looking_relative = tree_for_path("prefix/../C:folder/file.txt");

        assert_eq!(drive_rooted.root_hash(), drive_rooted_slashes.root_hash());
        assert_ne!(drive_rooted.root_hash(), drive_relative.root_hash());
        assert_ne!(drive_rooted.root_hash(), drive_looking_relative.root_hash());
        assert_ne!(
            drive_relative.root_hash(),
            drive_relative_looking_relative.root_hash()
        );
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
    fn deserialized_root_only_tree_does_not_claim_content_or_proof_data() {
        let files = make_test_files();
        let tree = MerkleTree::from_file_hashes(&files, Algorithm::Crc32);
        let restored = MerkleTree::from_bytes(&tree.to_bytes()).unwrap();

        assert_eq!(restored.file_count(), files.len());
        assert_eq!(restored.get_file_hash(0), None);
        assert!(!restored.verify_file(0, &files[0].hash));
        assert_eq!(restored.get_proof(0), None);
    }

    #[test]
    fn from_bytes_rejects_every_truncated_header_without_panicking() {
        let tree = tree_for_path("file.txt");
        let bytes = tree.to_bytes();

        for length in 0..9 {
            assert_eq!(MerkleTree::from_bytes(&bytes[..length]).map(|_| ()), None);
        }
    }

    #[test]
    fn from_bytes_rejects_truncated_declared_hash_without_panicking() {
        let mut bytes = vec![2];
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&32_u32.to_le_bytes());
        bytes.extend_from_slice(&[7; 31]);

        assert_eq!(MerkleTree::from_bytes(&bytes).map(|_| ()), None);
    }

    #[test]
    fn from_bytes_rejects_oversized_declared_hash_without_panicking() {
        let mut bytes = vec![2];
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());

        assert_eq!(MerkleTree::from_bytes(&bytes).map(|_| ()), None);
    }

    #[test]
    fn from_bytes_rejects_algorithm_specific_root_length_mismatches() {
        for (algorithm, expected_length) in [(0, 4), (1, 8), (2, 32)] {
            let short = serialized_root(algorithm, 1, &vec![7; expected_length - 1]);
            let long = serialized_root(algorithm, 1, &vec![7; expected_length + 1]);

            assert_eq!(MerkleTree::from_bytes(&short).map(|_| ()), None);
            assert_eq!(MerkleTree::from_bytes(&long).map(|_| ()), None);
        }
    }

    #[test]
    fn from_bytes_rejects_roots_inconsistent_with_file_count() {
        for (algorithm, expected_length) in [(0, 4), (1, 8), (2, 32)] {
            let nonempty_without_root = serialized_root(algorithm, 1, &[]);
            let empty_with_root = serialized_root(algorithm, 0, &vec![7; expected_length]);

            assert_eq!(
                MerkleTree::from_bytes(&nonempty_without_root).map(|_| ()),
                None
            );
            assert_eq!(MerkleTree::from_bytes(&empty_with_root).map(|_| ()), None);
        }
    }

    #[test]
    fn from_bytes_rejects_trailing_payload_bytes() {
        let mut bytes = serialized_root(2, 1, &[7; 32]);
        bytes.push(99);

        assert_eq!(MerkleTree::from_bytes(&bytes).map(|_| ()), None);
    }

    #[test]
    fn from_bytes_retains_maximum_count_without_allocating_capabilities() {
        let bytes = serialized_root(2, u32::MAX, &[7; 32]);
        let tree = MerkleTree::from_bytes(&bytes).unwrap();

        assert_eq!(tree.file_count(), u32::MAX as usize);
        assert!(tree.file_paths().is_empty());
        assert_eq!(tree.get_file_hash(0), None);
        assert_eq!(tree.get_proof(0), None);
        assert_eq!(tree.to_bytes(), bytes);
    }

    #[test]
    fn deserialized_root_only_tree_keeps_count_without_fabricating_paths() {
        let tree = MerkleTree::from_file_hashes(&make_test_files(), Algorithm::Crc32);
        let bytes = tree.to_bytes();
        let restored = MerkleTree::from_bytes(&bytes).unwrap();

        assert_eq!(restored.file_count(), tree.file_count());
        assert!(restored.file_paths().is_empty());
        assert_eq!(restored.to_bytes(), bytes);
    }

    #[test]
    fn test_empty_tree() {
        let tree = MerkleTree::from_file_hashes(&[], Algorithm::Crc32);
        assert_eq!(tree.file_count(), 0);
        assert!(tree.root_hash().bytes.is_empty());
    }
}
