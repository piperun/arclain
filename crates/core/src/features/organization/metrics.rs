use crate::features::organization::{engine::OrganizationPlan, GameMetadata};
use crate::ArchiveEntry;
use std::collections::HashSet;

/// Report of integrity statistics
#[derive(Debug, Clone, Default)]
pub struct IntegrityReport {
    pub original_files: usize,
    pub original_folders: usize,
    pub moved_files: usize,
    pub generated_files: usize,
    pub expected_screenshots: usize,
    pub planned_screenshots: usize,
    pub expected_modified_files: usize,
    pub file_discrepancy: i64,
    pub missing_original_files: Vec<String>,
    pub original_hash: u64,
    pub result_hash: u64,
    pub content_match: bool,
}

/// FNV-1a hash for fast fingerprinting
pub fn fnv1a_hash(data: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in data.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

impl IntegrityReport {
    pub fn calculate(
        entries: &[ArchiveEntry],
        plan: Option<&OrganizationPlan>,
        metadata: Option<&GameMetadata>,
    ) -> Self {
        let original_file_count = entries.iter().filter(|e| !e.is_dir).count();
        let original_folder_count = entries.iter().filter(|e| e.is_dir).count();

        let expected_screenshots = metadata.map(|m| m.screenshots.len()).unwrap_or(0);
        let planned_screenshots = plan.map(|p| p.downloads.len()).unwrap_or(0);
        let generated_files_count = plan.map(|p| p.generated_files.len()).unwrap_or(0);
        let moved_files = plan.map(|p| p.moves.len()).unwrap_or(0);

        let expected_modified_files = moved_files + generated_files_count + planned_screenshots;

        // 1. Original file paths
        let mut original_set = HashSet::new();
        // Add only files, not folders? Tree walk included folders only if explicitly named?
        // Original logic: collect_full_paths walks tree. If node is_dir, recurses. else result.insert.
        // So it only collects files.
        for entry in entries {
            if !entry.is_dir {
                // Determine path. ArchiveEntry path usually uses forward slashes or platform specific?
                // We should normalize.
                let path = entry.path.replace('\\', "/");
                original_set.insert(path);
            }
        }

        let mut original_paths: Vec<String> = original_set.iter().cloned().collect();
        original_paths.sort();
        let original_hash = fnv1a_hash(&original_paths.join("|"));

        // 2. Covered paths from plan
        let mut plan_sources: Vec<String> = if let Some(p) = plan {
            p.moves
                .iter()
                .map(|(src, _)| src.replace('\\', "/"))
                .collect()
        } else {
            Vec::new()
        };

        let covered_set: HashSet<String> = plan_sources.iter().cloned().collect();
        let missing_original_files: Vec<String> =
            original_set.difference(&covered_set).cloned().collect();

        plan_sources.sort();
        let result_hash = fnv1a_hash(&plan_sources.join("|"));

        let content_match = original_hash == result_hash;

        // Discrepancy logic
        // "modified_file_count" in UI came from self.organized_tree.
        // Here we don't build organized tree yet.
        // But organized file count is roughly: moved_files + generated_files + planned_screenshots
        // UNLESS we dropped some original files (missing originals).
        // Let's approximate expected total vs theoretical total.
        // Or if we can't calculate file_discrepancy exactly without tree sim, we can output 0 or calc based on moves.
        // Actually, if we use (moved + generated + screenshots), that IS the count of the organized structure if no overwrites/collisions.
        // Let's assume ideal state for this check.
        let organized_file_count = moved_files + generated_files_count + planned_screenshots; // Approximation

        let expected_total = original_file_count + generated_files_count + planned_screenshots;
        // If content_match is true, moved_files == original_files.
        // So file_discrepancy = organized - expected.
        let file_discrepancy = (organized_file_count as i64) - (expected_total as i64);

        Self {
            original_files: original_file_count,
            original_folders: original_folder_count,
            moved_files,
            generated_files: generated_files_count,
            expected_screenshots,
            planned_screenshots,
            expected_modified_files,
            file_discrepancy,
            missing_original_files,
            original_hash,
            result_hash,
            content_match,
        }
    }
}
