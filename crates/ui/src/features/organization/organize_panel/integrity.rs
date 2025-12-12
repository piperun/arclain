//! Integrity verification for organization plans
//!
//! Calculates discrepancies between original archive and organized output.

use crate::shared::components::preview_tree::{self, PreviewTreeNode};

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

/// Count files in a PreviewTreeNode tree (recursive)
pub fn count_files(nodes: &[PreviewTreeNode]) -> usize {
    let mut count = 0;
    for node in nodes {
        if node.is_dir {
            count += count_files(&node.children);
        } else {
            count += 1;
        }
    }
    count
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

/// Count folders in a PreviewTreeNode tree (recursive)
pub fn count_folders(nodes: &[PreviewTreeNode]) -> usize {
    let mut count = 0;
    for node in nodes {
        if node.is_dir {
            count += 1;
            count += count_folders(&node.children);
        }
    }
    count
}

/// Collect all file full paths from a tree
pub fn collect_full_paths(
    nodes: &[preview_tree::PreviewTreeNode],
    result: &mut std::collections::HashSet<String>,
    prefix: &str,
) {
    for node in nodes {
        let path = if prefix.is_empty() {
            node.name.clone()
        } else {
            format!("{}/{}", prefix, node.name)
        };

        if node.is_dir {
            collect_full_paths(&node.children, result, &path);
        } else {
            result.insert(path);
        }
    }
}

/// Export a report of all discrepancies (files filtered out, missing screenshots, etc.)
pub fn export_issues_report(
    report: &IntegrityReport,
    _original_tree: &[preview_tree::PreviewTreeNode],
    _organized_tree: &[preview_tree::PreviewTreeNode],
    metadata: &Option<arclain_core::features::organization::GameMetadata>,
) {
    use std::io::Write;

    let mut content = String::new();
    content.push_str("# Organization Issues Report\n\n");
    content.push_str(&format!(
        "Generated: {}\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));

    content.push_str("## Summary\n\n");
    content.push_str(&format!("- Original files: {}\n", report.original_files));
    content.push_str(&format!("- Moved files: {}\n", report.moved_files));
    content.push_str(&format!("- Generated files: {}\n", report.generated_files));
    content.push_str(&format!(
        "- Expected screenshots: {}\n",
        report.expected_screenshots
    ));
    content.push_str(&format!(
        "- Planned screenshots: {}\n",
        report.planned_screenshots
    ));
    content.push_str(&format!(
        "- File discrepancy: {}\n",
        report.file_discrepancy
    ));
    content.push_str(&format!("- Content Verified: {}\n", report.content_match));
    content.push_str(&format!("- Original Hash: {:016x}\n", report.original_hash));
    content.push_str(&format!("- Result Hash:   {:016x}\n\n", report.result_hash));

    if !report.missing_original_files.is_empty() {
        content.push_str("## Files Not Covered by Plan\n\n");
        content.push_str(
            "These files from the original archive are not included in the move/copy plan and will be lost:\n\n",
        );
        for path in &report.missing_original_files {
            content.push_str(&format!("- `{}`\n", path));
        }
        content.push('\n');
    }

    // Screenshot issues
    if report.expected_screenshots != report.planned_screenshots {
        content.push_str("## Screenshot Issues\n\n");

        if let Some(meta) = metadata {
            let expected_urls: std::collections::HashSet<_> =
                meta.screenshots.iter().cloned().collect();

            content.push_str(&format!(
                "Expected {} screenshots from metadata, but only {} are planned for download.\n\n",
                report.expected_screenshots, report.planned_screenshots
            ));
            content.push_str("Expected screenshot identifiers from metadata:\n\n");

            for data in expected_urls {
                let identifier = match data {
                    arclain_core::features::organization::ScreenshotData::FilePath(p) => {
                        p.display().to_string()
                    }
                    arclain_core::features::organization::ScreenshotData::Base64(s) => {
                        format!("Base64 data ({} bytes)", s.len())
                    }
                };
                content.push_str(&format!("- `{}`\n", identifier));
            }
        }
        content.push('\n');
    }

    // Save to file
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let filename = format!("organization_issues_{}.md", timestamp);

    if let Some(path) = rfd::FileDialog::new()
        .set_file_name(&filename)
        .add_filter("Markdown", &["md"])
        .save_file()
    {
        if let Ok(mut file) = std::fs::File::create(&path) {
            if let Err(e) = file.write_all(content.as_bytes()) {
                tracing::error!("Failed to write issues report: {}", e);
            } else {
                tracing::info!("Issues report exported to {:?}", path);
                // Try to open the file
                if let Err(e) = open::that(&path) {
                    tracing::warn!("Failed to open exported file: {}", e);
                }
            }
        }
    }
}
