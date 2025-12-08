//! Preview Tree Component
//!
//! A reusable component for rendering hierarchical file/folder structures
//! in the organize preview panel. Supports both original and organized views.

use arclain_core::organization::engine::PendingDownload;
use eframe::egui::{self, RichText, Ui};
use std::collections::HashMap;

/// Filter for what to show in the preview tree
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum PreviewFilter {
    #[default]
    All,
    FoldersOnly,
    FilesOnly,
    GeneratedOnly,
}

/// A node in the preview tree
#[derive(Debug, Clone, serde::Serialize)]
#[allow(dead_code)]
pub struct PreviewTreeNode {
    pub name: String,
    pub full_path: String,
    pub is_dir: bool,
    pub is_generated: bool,
    pub is_download: bool,
    pub children: Vec<PreviewTreeNode>,
    pub file_count: usize, // For directories: count of files inside
}

#[allow(dead_code)]
impl PreviewTreeNode {
    pub fn new_file(name: String, full_path: String) -> Self {
        Self {
            name,
            full_path,
            is_dir: false,
            is_generated: false,
            is_download: false,
            children: Vec::new(),
            file_count: 0,
        }
    }

    pub fn new_folder(name: String, full_path: String) -> Self {
        Self {
            name,
            full_path,
            is_dir: true,
            is_generated: false,
            is_download: false,
            children: Vec::new(),
            file_count: 0,
        }
    }

    pub fn generated(mut self) -> Self {
        self.is_generated = true;
        self
    }

    pub fn download(mut self) -> Self {
        self.is_download = true;
        self
    }
}

/// State for the preview tree (expansion, etc.)
#[derive(Default, Clone)]
pub struct PreviewTreeState {
    pub expanded_folders: HashMap<String, bool>,
}

#[allow(dead_code)]
impl PreviewTreeState {
    pub fn expand_all(&mut self, nodes: &[PreviewTreeNode]) {
        for node in nodes {
            if node.is_dir {
                self.expanded_folders.insert(node.full_path.clone(), true);
                self.expand_all(&node.children);
            }
        }
    }

    pub fn collapse_all(&mut self) {
        self.expanded_folders.clear();
    }

    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded_folders.get(path).copied().unwrap_or(true) // Default expanded
    }

    pub fn toggle(&mut self, path: &str) {
        let current = self.is_expanded(path);
        self.expanded_folders.insert(path.to_string(), !current);
    }
}

/// Build a tree structure from a list of file paths
pub fn build_tree_from_paths(paths: &[(String, bool, bool)]) -> Vec<PreviewTreeNode> {
    // paths: (full_path, is_generated, is_download)
    // Use a simpler approach: build a flat map of all nodes first, then assemble tree

    let mut nodes: HashMap<String, PreviewTreeNode> = HashMap::new();

    for (path, is_generated, is_download) in paths {
        // Handle both / and \ path separators (Windows vs Unix)
        let parts: Vec<&str> = path
            .split(|c| c == '/' || c == '\\')
            .filter(|s| !s.is_empty())
            .collect();

        if parts.is_empty() {
            continue;
        }

        // Build all ancestor folders first
        let mut current_path = String::new();
        for (i, part) in parts.iter().enumerate() {
            if !current_path.is_empty() {
                current_path.push('/');
            }
            current_path.push_str(part);

            let is_last = i == parts.len() - 1;

            if is_last {
                // This is the file - create or update it
                let mut node = PreviewTreeNode::new_file(part.to_string(), current_path.clone());
                if *is_generated {
                    node.is_generated = true;
                }
                if *is_download {
                    node.is_download = true;
                }
                nodes.insert(current_path.clone(), node);
            } else {
                // This is a folder - create if doesn't exist
                nodes.entry(current_path.clone()).or_insert_with(|| {
                    PreviewTreeNode::new_folder(part.to_string(), current_path.clone())
                });
            }
        }
    }

    // Now build the tree structure by linking children to parents
    let all_paths: Vec<String> = nodes.keys().cloned().collect();

    for path in &all_paths {
        // Find parent path
        if let Some(last_sep) = path.rfind('/') {
            let parent_path = &path[..last_sep];
            if let Some(child) = nodes.get(path).cloned() {
                if let Some(parent) = nodes.get_mut(parent_path) {
                    if !parent
                        .children
                        .iter()
                        .any(|c| c.full_path == child.full_path)
                    {
                        parent.children.push(child);
                    }
                }
            }
        }
    }

    // Collect only root nodes (those without a parent in our map)
    let mut result: Vec<PreviewTreeNode> = nodes
        .into_iter()
        .filter(|(path, _)| !path.contains('/'))
        .map(|(_, node)| node)
        .collect();

    sort_tree(&mut result);
    result
}

/// Create a new filtered tree based on the provided filter
#[allow(dead_code)]
pub fn filter_tree(nodes: &[PreviewTreeNode], filter: PreviewFilter) -> Vec<PreviewTreeNode> {
    let mut result = Vec::new();

    for node in nodes {
        let should_include_node = match filter {
            PreviewFilter::All => true,
            PreviewFilter::FoldersOnly => node.is_dir,
            PreviewFilter::FilesOnly => !node.is_dir,
            PreviewFilter::GeneratedOnly => node.is_generated || node.is_download,
        };

        // If it's a folder, we might need to include it if it has matching children, even if the filter says folders only?
        // Logic: Recursively filter children.
        // If folders only: include folders. Files inside? No.
        // If files only: include files. Folders are needed to show structure? User said "folder only", "files + folders".
        // Usually "Files Only" means flat list or still structured? "Folders Only" definitely means structure.
        // Let's stick to the simple logic used in render:

        let mut new_node = node.clone();
        new_node.children = filter_tree(&node.children, filter);

        if should_include_node {
            result.push(new_node);
        } else if node.is_dir && !new_node.children.is_empty() {
            // Keep directory if it has matching children (e.g. for FilesOnly view, we need path?)
            // Actually, render_node logic hides the node if !should_show.
            // But if it's a directory with children that ARE shown, does render_node show it?
            // render_node: if !should_show && !node.is_dir { return; }
            // This implies if it IS a dir, it's shown unless explicitly excluded elsewhere?
            // Actually "FoldersOnly" -> node.is_dir is true.
            // "FilesOnly" -> node.is_dir is false.
            // If Filter is FilesOnly, render_node hits !should_show (false) && !node.is_dir (false) => passes through for dirs?
            // Let's match render_node:

            // If node is a directory, we keep it if it has children after filtering, OR if the filter specifically includes directories?
            // Render logic says: if !should_show && !node.is_dir { return } -> Files are hidden if filter doesn't match. Directories are effectively "always shown" to traverse?
            // But wait, allow filtering for export to be simpler:

            if !new_node.children.is_empty() {
                result.push(new_node);
            }
        }
    }
    result
}

fn sort_tree(nodes: &mut [PreviewTreeNode]) {
    nodes.sort_by(|a, b| {
        // Folders first, then files
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    for node in nodes.iter_mut() {
        sort_tree(&mut node.children);
    }
}

/// Render the preview tree
pub fn render_tree(
    ui: &mut Ui,
    state: &mut PreviewTreeState,
    nodes: &[PreviewTreeNode],
    filter: PreviewFilter,
    depth_limit: Option<usize>,
) {
    // Ensure vertical layout for tree items
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0; // Compact spacing between rows
        for node in nodes {
            render_node(ui, state, node, 0, filter, depth_limit);
        }
    });
}

fn render_node(
    ui: &mut Ui,
    state: &mut PreviewTreeState,
    node: &PreviewTreeNode,
    indent: usize,
    filter: PreviewFilter,
    depth_limit: Option<usize>,
) {
    // Apply depth limit
    if let Some(limit) = depth_limit {
        if indent > limit {
            return;
        }
    }

    // Apply filter
    let should_show = match filter {
        PreviewFilter::All => true,
        PreviewFilter::FoldersOnly => node.is_dir,
        PreviewFilter::FilesOnly => !node.is_dir,
        PreviewFilter::GeneratedOnly => node.is_generated || node.is_download,
    };

    if !should_show && !node.is_dir {
        return;
    }

    let indent_px = indent as f32 * 16.0;
    let is_expanded = state.is_expanded(&node.full_path);

    ui.horizontal(|ui| {
        ui.add_space(indent_px);

        // Expand/collapse button for folders
        if node.is_dir && !node.children.is_empty() {
            let arrow = if is_expanded { "▼" } else { "▶" };
            if ui
                .add(egui::Button::new(RichText::new(arrow).size(10.0)).frame(false))
                .clicked()
            {
                state.toggle(&node.full_path);
            }
        } else {
            ui.add_space(16.0);
        }

        // Icon
        let icon = if node.is_dir {
            if is_expanded {
                egui_phosphor::regular::FOLDER_OPEN
            } else {
                egui_phosphor::regular::FOLDER
            }
        } else {
            egui_phosphor::regular::FILE
        };

        let icon_color = if node.is_dir {
            egui::Color32::from_rgb(250, 204, 21) // Yellow for folders
        } else if node.is_generated {
            egui::Color32::from_rgb(250, 204, 21) // Yellow for generated
        } else if node.is_download {
            egui::Color32::from_rgb(147, 197, 253) // Blue for downloads
        } else {
            egui::Color32::from_rgb(156, 163, 175) // Gray for files
        };

        ui.label(RichText::new(icon).size(14.0).color(icon_color));

        // Name
        let name_text = RichText::new(&node.name).size(12.0);
        let response = ui.label(name_text);

        // Generated/download badge
        if node.is_generated {
            ui.label(
                RichText::new("✨")
                    .size(10.0)
                    .color(egui::Color32::from_rgb(250, 204, 21)),
            );
        } else if node.is_download {
            ui.label(
                RichText::new("📥")
                    .size(10.0)
                    .color(egui::Color32::from_rgb(147, 197, 253)),
            );
        }

        // Copy path on click
        if response.clicked() {
            ui.ctx().copy_text(node.full_path.clone());
        }

        // Tooltip with full path
        response.on_hover_text(&node.full_path);
    });

    // Render children if expanded
    if node.is_dir && is_expanded {
        for child in &node.children {
            render_node(ui, state, child, indent + 1, filter, depth_limit);
        }
    }
}

/// Build tree from OrganizationPlan moves
pub fn build_organized_tree(
    moves: &[(String, String)],
    generated_files: &[(String, String)],
    downloads: &[PendingDownload],
    resolved_variables: &HashMap<String, String>,
) -> Vec<PreviewTreeNode> {
    let mut paths: Vec<(String, bool, bool)> = Vec::new();

    // Helper to replace variables in path
    let resolve_path = |path: &str| -> String {
        let mut resolved = path.to_string();
        for (var, val) in resolved_variables {
            resolved = resolved.replace(var, val);
        }
        resolved
    };

    // Add moved files (destination paths)
    for (_, dst) in moves {
        paths.push((resolve_path(dst), false, false));
    }

    // Add generated files
    for (path, _) in generated_files {
        paths.push((resolve_path(path), true, false));
    }

    // Add downloads
    for download in downloads {
        paths.push((resolve_path(&download.dest_path), false, true));
    }

    build_tree_from_paths(&paths)
}

/// Build tree from original archive entries
pub fn build_original_tree(entries: &[String]) -> Vec<PreviewTreeNode> {
    let paths: Vec<(String, bool, bool)> =
        entries.iter().map(|p| (p.clone(), false, false)).collect();
    build_tree_from_paths(&paths)
}
