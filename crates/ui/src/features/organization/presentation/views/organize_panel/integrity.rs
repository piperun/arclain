//! Integrity verification for organization plans
//!
//! Calculates discrepancies between original archive and organized output.

use crate::shared::components::preview_tree;
use arclain_app::archive::ProductMetadataSummary;
use arclain_app::organization::OrganizeIntegrityDto;

// Helper functions removed as logic is now in core

/// Export a report of all discrepancies (files filtered out, missing screenshots, etc.)
///
/// Every number here comes from the facade's own integrity DTO, and the
/// screenshot identifiers from the facade's product-metadata summary --
/// which already enumerates each distinct screenshot once, in the order
/// the plugin reported them, and can name one without its bytes.
pub fn export_issues_report(
    report: &OrganizeIntegrityDto,
    _original_tree: &[preview_tree::PreviewTreeNode],
    _organized_tree: &[preview_tree::PreviewTreeNode],
    metadata: Option<&ProductMetadataSummary>,
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
            content.push_str(&format!(
                "Expected {} screenshots from metadata, but only {} are planned for download.\n\n",
                report.expected_screenshots, report.planned_screenshots
            ));
            content.push_str("Expected screenshot identifiers from metadata:\n\n");

            for screenshot in &meta.screenshots {
                content.push_str(&format!("- `{}`\n", screenshot.identifier()));
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
