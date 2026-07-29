//! Integrity verification for organization plans
//!
//! Calculates discrepancies between original archive and organized output.

use crate::shared::components::preview_tree;
use arclain_app::organization::OrganizeIntegrityDto;

// Helper functions removed as logic is now in core

/// Export a report of all discrepancies (files filtered out, missing screenshots, etc.)
///
/// **Boundary note:** `metadata` is the one `arclain_core` type left in
/// the organize panel. The report enumerates the *identifiers* of the
/// screenshots a plan did not schedule, and no facade method exposes
/// those yet -- the metadata read model is a later task's surface to
/// design, so this one read stays where it is rather than being guessed
/// at here. Every number in the report comes from the facade's own
/// integrity DTO.
pub fn export_issues_report(
    report: &OrganizeIntegrityDto,
    _original_tree: &[preview_tree::PreviewTreeNode],
    _organized_tree: &[preview_tree::PreviewTreeNode],
    metadata: Option<&arclain_core::features::organization::GameMetadata>,
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
                tracing::info!("Issues report exported to {}", path.display());
                // Try to open the file
                if let Err(e) = open::that(&path) {
                    tracing::warn!("Failed to open exported file: {}", e);
                }
            }
        }
    }
}
