use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct ContentHashMap {
    pub hashes: BTreeMap<String, String>,
    pub root_hash: String,
}

impl ContentHashMap {
    pub fn from_directory(root: &Path) -> anyhow::Result<Self> {
        let mut hashes = BTreeMap::new();

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let rel_path = entry
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");

            let content = fs::read(entry.path())?;
            let file_hash = format!("{:x}", Sha256::digest(&content));

            hashes.insert(rel_path, file_hash);
        }

        let root_hash = compute_merkle_root(&hashes);

        Ok(Self { hashes, root_hash })
    }

    pub fn compare(&self, other: &Self) -> ContentComparison {
        let mut missing_files = Vec::new();
        let mut extra_files = Vec::new();
        let mut modified_files = Vec::new();

        for (path, hash) in &self.hashes {
            match other.hashes.get(path) {
                None => missing_files.push(path.clone()),
                Some(other_hash) if other_hash != hash => {
                    modified_files.push((path.clone(), hash.clone(), other_hash.clone()));
                }
                _ => {}
            }
        }

        for path in other.hashes.keys() {
            if !self.hashes.contains_key(path) {
                extra_files.push(path.clone());
            }
        }

        ContentComparison {
            root_hash_matches: self.root_hash == other.root_hash,
            expected_root: self.root_hash.clone(),
            actual_root: other.root_hash.clone(),
            missing_files,
            extra_files,
            modified_files,
        }
    }

    pub fn print_summary(&self) {
        println!("Content Hash Map:");
        println!("  Root hash: {}", self.root_hash);
        println!("  Total files: {}", self.hashes.len());

        let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
        for path in self.hashes.keys() {
            let dir = if let Some(pos) = path.rfind('/') {
                &path[..pos]
            } else {
                "."
            };
            *dirs.entry(dir.to_string()).or_insert(0) += 1;
        }

        println!("  Directories:");
        for (dir, count) in dirs.iter().take(10) {
            println!("    {}: {} files", dir, count);
        }
        if dirs.len() > 10 {
            println!("    ... and {} more directories", dirs.len() - 10);
        }
    }
}

#[derive(Debug)]
pub struct ContentComparison {
    pub root_hash_matches: bool,
    pub expected_root: String,
    pub actual_root: String,
    pub missing_files: Vec<String>,
    pub extra_files: Vec<String>,
    pub modified_files: Vec<(String, String, String)>,
}

impl ContentComparison {
    pub fn is_exact_match(&self) -> bool {
        self.root_hash_matches
            && self.missing_files.is_empty()
            && self.extra_files.is_empty()
            && self.modified_files.is_empty()
    }

    pub fn print_report(&self) {
        println!("\n=== Content Verification Report ===");
        println!(
            "Root Hash Match: {}",
            if self.root_hash_matches {
                "✓ YES"
            } else {
                "✗ NO"
            }
        );
        println!("  Expected: {}", self.expected_root);
        println!("  Actual:   {}", self.actual_root);
        println!();

        if !self.missing_files.is_empty() {
            println!("Missing Files ({}):", self.missing_files.len());
            for (i, path) in self.missing_files.iter().take(20).enumerate() {
                println!("  {}. {}", i + 1, path);
            }
            if self.missing_files.len() > 20 {
                println!("  ... and {} more", self.missing_files.len() - 20);
            }
            println!();
        }

        if !self.extra_files.is_empty() {
            println!("Extra Files ({}):", self.extra_files.len());
            for (i, path) in self.extra_files.iter().take(20).enumerate() {
                println!("  {}. {}", i + 1, path);
            }
            if self.extra_files.len() > 20 {
                println!("  ... and {} more", self.extra_files.len() - 20);
            }
            println!();
        }

        if !self.modified_files.is_empty() {
            println!("Modified Files ({}):", self.modified_files.len());
            for (i, (path, expected, actual)) in self.modified_files.iter().take(10).enumerate() {
                println!("  {}. {}", i + 1, path);
                println!("     Expected: {}", expected);
                println!("     Actual:   {}", actual);
            }
            if self.modified_files.len() > 10 {
                println!("  ... and {} more", self.modified_files.len() - 10);
            }
            println!();
        }

        println!("===================================\n");
    }
}

fn compute_merkle_root(hashes: &BTreeMap<String, String>) -> String {
    if hashes.is_empty() {
        return format!("{:x}", Sha256::digest(b""));
    }

    let mut sorted_hashes: Vec<(&String, &String)> = hashes.iter().collect();
    sorted_hashes.sort_by_key(|(path, _)| *path);

    let mut level: Vec<String> = sorted_hashes
        .iter()
        .map(|(path, hash)| {
            let mut hasher = Sha256::new();
            hasher.update(path.as_bytes());
            hasher.update(b":");
            hasher.update(hash.as_bytes());
            format!("{:x}", hasher.finalize())
        })
        .collect();

    while level.len() > 1 {
        let mut next_level = Vec::new();

        for chunk in level.chunks(2) {
            let combined = if chunk.len() == 2 {
                format!("{}{}", chunk[0], chunk[1])
            } else {
                format!("{}{}", chunk[0], chunk[0])
            };

            let hash = format!("{:x}", Sha256::digest(combined.as_bytes()));
            next_level.push(hash);
        }

        level = next_level;
    }

    level[0].clone()
}
