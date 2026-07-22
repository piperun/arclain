use crate::shared::components::preview_tree::PreviewTreeNode;
use arclain_core::features::organization::GameMetadata;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportTreeMode {
    Original,
    Modified,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Text,
    Json,
}

pub struct ExportTreeDialog {
    is_open: bool,
    mode: ExportTreeMode,
    format: ExportFormat,
    include_metadata: bool,
}

impl Default for ExportTreeDialog {
    fn default() -> Self {
        Self {
            is_open: false,
            mode: ExportTreeMode::Both,
            format: ExportFormat::Text,
            include_metadata: true,
        }
    }
}

impl ExportTreeDialog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self) {
        self.is_open = true;
    }

    pub fn close(&mut self) {
        self.is_open = false;
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        original_tree: &[PreviewTreeNode],
        organized_tree: &[PreviewTreeNode],
        metadata: Option<&GameMetadata>,
    ) {
        if !self.is_open {
            return;
        }

        let mut is_open = self.is_open;
        egui::Window::new("Export Tree Structure")
            .open(&mut is_open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(340.0);

                ui.add_space(8.0);
                ui.label("Select which tree structure and format to export.");
                ui.add_space(12.0);

                ui.group(|ui| {
                    ui.label(egui::RichText::new("Tree Selection").strong().size(12.0));
                    ui.add_space(4.0);

                    ui.radio_value(
                        &mut self.mode,
                        ExportTreeMode::Both,
                        "Both (Original & Modified)",
                    )
                    .on_hover_text("Export both structures for comparison");

                    ui.radio_value(
                        &mut self.mode,
                        ExportTreeMode::Modified,
                        "Modified Tree (Simulation)",
                    )
                    .on_hover_text("Export the planned structure after organization");

                    ui.radio_value(&mut self.mode, ExportTreeMode::Original, "Original Tree")
                        .on_hover_text("Export the current structure of the archive");
                });

                ui.add_space(8.0);

                ui.group(|ui| {
                    ui.label(egui::RichText::new("Export Format").strong().size(12.0));
                    ui.add_space(4.0);

                    ui.radio_value(
                        &mut self.format,
                        ExportFormat::Text,
                        "Text (Human-readable)",
                    )
                    .on_hover_text("Plain text with indented tree structure");

                    ui.radio_value(
                        &mut self.format,
                        ExportFormat::Json,
                        "JSON (Machine-readable)",
                    )
                    .on_hover_text("JSON format for programmatic processing");
                });

                ui.add_space(8.0);

                if metadata.is_some() {
                    ui.checkbox(&mut self.include_metadata, "Include Metadata")
                        .on_hover_text("Include the full metadata in the export");
                } else {
                    ui.add_enabled(false, egui::Checkbox::new(&mut false, "Include Metadata"))
                        .on_hover_text("No metadata available for this archive");
                }

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    if ui
                        .add(TextButton::new("Cancel", ButtonSize::Small))
                        .clicked()
                    {
                        self.close();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(TextButton::new("Export...", ButtonSize::Medium))
                            .clicked()
                        {
                            self.perform_export(original_tree, organized_tree, metadata);
                            self.close();
                        }
                    });
                });

                ui.add_space(4.0);
            });

        self.is_open = is_open;
    }

    fn perform_export(
        &self,
        original_tree: &[PreviewTreeNode],
        organized_tree: &[PreviewTreeNode],
        metadata: Option<&GameMetadata>,
    ) {
        let (content, extension, filter_name) = match self.format {
            ExportFormat::Text => (
                self.generate_text_export(original_tree, organized_tree, metadata),
                "txt",
                "Text File",
            ),
            ExportFormat::Json => (
                self.generate_json_export(original_tree, organized_tree, metadata),
                "json",
                "JSON File",
            ),
        };

        let filename = format!("archive_structure.{}", extension);

        let task = rfd::FileDialog::new()
            .set_file_name(&filename)
            .add_filter(filter_name, &[extension])
            .save_file();

        if let Some(path) = task {
            if let Err(e) = std::fs::write(&path, content) {
                tracing::error!("Failed to save export file: {}", e);
            } else {
                tracing::info!("Exported tree structure to {}", path.display());
                if let Err(e) = open::that(path) {
                    tracing::warn!("Failed to open exported file: {}", e);
                }
            }
        }
    }

    fn generate_text_export(
        &self,
        original_tree: &[PreviewTreeNode],
        organized_tree: &[PreviewTreeNode],
        metadata: Option<&GameMetadata>,
    ) -> String {
        let mut content = String::new();

        if self.mode == ExportTreeMode::Original || self.mode == ExportTreeMode::Both {
            content.push_str("=== ORIGINAL STRUCTURE ===\n\n");
            content.push_str(&self.render_tree_text(original_tree, 0));
            content.push_str("\n");
        }

        if self.mode == ExportTreeMode::Both {
            content.push_str("\n");
        }

        if self.mode == ExportTreeMode::Modified || self.mode == ExportTreeMode::Both {
            content.push_str("=== MODIFIED STRUCTURE ===\n\n");
            content.push_str(&self.render_tree_text(organized_tree, 0));
            content.push_str("\n");
        }

        if self.include_metadata {
            if let Some(meta) = metadata {
                content.push_str("\n=== METADATA ===\n\n");
                if let Ok(json) = serde_json::to_string_pretty(meta) {
                    content.push_str(&json);
                } else {
                    content.push_str(&format!("{:#?}", meta));
                }
                content.push_str("\n");
            }
        }

        content
    }

    fn generate_json_export(
        &self,
        original_tree: &[PreviewTreeNode],
        organized_tree: &[PreviewTreeNode],
        metadata: Option<&GameMetadata>,
    ) -> String {
        #[derive(Serialize)]
        struct ExportData<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            original_tree: Option<&'a [PreviewTreeNode]>,
            #[serde(skip_serializing_if = "Option::is_none")]
            modified_tree: Option<&'a [PreviewTreeNode]>,
            #[serde(skip_serializing_if = "Option::is_none")]
            metadata: Option<&'a GameMetadata>,
        }

        let data = ExportData {
            original_tree: if self.mode == ExportTreeMode::Original
                || self.mode == ExportTreeMode::Both
            {
                Some(original_tree)
            } else {
                None
            },
            modified_tree: if self.mode == ExportTreeMode::Modified
                || self.mode == ExportTreeMode::Both
            {
                Some(organized_tree)
            } else {
                None
            },
            metadata: if self.include_metadata {
                metadata
            } else {
                None
            },
        };

        serde_json::to_string_pretty(&data).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
    }

    fn render_tree_text(&self, nodes: &[PreviewTreeNode], depth: usize) -> String {
        let mut out = String::new();
        let indent = "  ".repeat(depth);

        for node in nodes {
            if node.is_dir {
                out.push_str(&format!("{}{}/\n", indent, node.name));
                out.push_str(&self.render_tree_text(&node.children, depth + 1));
            } else {
                out.push_str(&format!("{}{}\n", indent, node.name));
            }
        }
        out
    }
}
