use crate::shared::theme::AppTheme;
use arclain_widgets::pixel_align;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct TreePanelState {
    pub selected_path: String,
    expanded_folders: Arc<HashSet<String>>,
    expansion_generation: u64,
}

impl PartialEq for TreePanelState {
    fn eq(&self, other: &Self) -> bool {
        self.selected_path == other.selected_path
            && self.expansion_generation == other.expansion_generation
    }
}

impl TreePanelState {
    fn set_selected_path(&mut self, current_path: &str) -> bool {
        if self.selected_path == current_path {
            return false;
        }
        self.selected_path.clear();
        self.selected_path.push_str(current_path);
        true
    }

    fn is_expanded(&self, path: &str) -> bool {
        self.expanded_folders.contains(path)
    }

    fn bump_expansion_generation(&mut self) {
        self.expansion_generation = self.expansion_generation.wrapping_add(1).max(1);
    }

    pub(crate) fn expansion_generation(&self) -> u64 {
        self.expansion_generation
    }

    pub(crate) fn toggle_expanded(&mut self, path: &str) -> bool {
        if path.is_empty() {
            return false;
        }
        let expanded = Arc::make_mut(&mut self.expanded_folders);
        if !expanded.remove(path) {
            expanded.insert(path.to_string());
        }
        self.bump_expansion_generation();
        true
    }

    pub(crate) fn auto_expand_current_path(&mut self, current_path: &str) -> bool {
        if current_path.is_empty() {
            return false;
        }

        let mut path_accumulator = String::new();
        let mut missing = Vec::new();
        for segment in current_path.split('/') {
            if !path_accumulator.is_empty() {
                path_accumulator.push('/');
            }
            path_accumulator.push_str(segment);
            if !self.expanded_folders.contains(&path_accumulator) {
                missing.push(path_accumulator.clone());
            }
        }

        if missing.is_empty() {
            return false;
        }
        Arc::make_mut(&mut self.expanded_folders).extend(missing);
        self.bump_expansion_generation();
        true
    }
}

#[derive(Debug, Clone)]
struct TreeNode {
    name: String,
    full_path: String,
    children: Vec<TreeNode>,
    indent_level: usize,
}

impl TreeNode {
    fn new(name: String, full_path: String, indent_level: usize) -> Self {
        Self {
            name,
            full_path,
            children: Vec::new(),
            indent_level,
        }
    }
}

/// Immutable folder hierarchy prepared outside egui's render callbacks.
///
/// Building the hierarchy allocates and sorts, so archive-browser renderers
/// retain one of these per tab and rebuild it only when the archive entry
/// allocation changes.
#[derive(Debug, Clone, Default)]
pub struct FolderTree {
    roots: Vec<TreeNode>,
}

impl FolderTree {
    pub fn from_folders(folders: &[String]) -> Self {
        let mut root_nodes: Vec<TreeNode> = Vec::new();
        let mut folder_map: HashMap<String, Vec<String>> = HashMap::new();

        // Build parent-child relationships
        for folder in folders {
            if let Some(pos) = folder.rfind('/') {
                let parent = &folder[..pos];
                folder_map
                    .entry(parent.to_string())
                    .or_default()
                    .push(folder.clone());
            } else {
                // Top-level folder
                folder_map
                    .entry(String::new())
                    .or_default()
                    .push(folder.clone());
            }
        }

        // Build tree recursively
        fn build_node(
            path: &str,
            folder_map: &HashMap<String, Vec<String>>,
            indent: usize,
        ) -> TreeNode {
            let name = if let Some(pos) = path.rfind('/') {
                path[pos + 1..].to_string()
            } else {
                path.to_string()
            };

            let mut node = TreeNode::new(name, path.to_string(), indent);

            if let Some(children) = folder_map.get(path) {
                for child in children {
                    node.children
                        .push(build_node(child, folder_map, indent + 1));
                }
            }

            node.children.sort_by(|a, b| a.name.cmp(&b.name));
            node
        }

        if let Some(top_level) = folder_map.get("") {
            for folder in top_level {
                root_nodes.push(build_node(folder, &folder_map, 1));
            }
        }

        root_nodes.sort_by(|a, b| a.name.cmp(&b.name));
        Self { roots: root_nodes }
    }

    fn flatten_visible(&self, state: &TreePanelState, rows: &mut Vec<TreeRow>) {
        rows.push(TreeRow::Root);
        for node in &self.roots {
            Self::flatten_node(node, state, rows);
        }
    }

    fn flatten_node(node: &TreeNode, state: &TreePanelState, rows: &mut Vec<TreeRow>) {
        let has_children = !node.children.is_empty();
        rows.push(TreeRow::Folder {
            name: node.name.clone(),
            full_path: node.full_path.clone(),
            indent_level: node.indent_level,
            has_children,
        });
        if has_children && state.is_expanded(&node.full_path) {
            for child in &node.children {
                Self::flatten_node(child, state, rows);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeRow {
    Root,
    Folder {
        name: String,
        full_path: String,
        indent_level: usize,
        has_children: bool,
    },
}

impl TreeRow {
    #[cfg(test)]
    fn path_for_test(&self) -> &str {
        match self {
            Self::Root => "",
            Self::Folder { full_path, .. } => full_path,
        }
    }
}

/// Cached visible tree rows for one archive tab.
///
/// The hierarchy generation and expansion generation are explicit O(1) keys;
/// settled frames neither clone/compare the expansion set nor rebuild row
/// metadata.
#[derive(Default)]
pub struct TreeRowProjectionCache {
    tree_generation: Option<u64>,
    expansion_generation: u64,
    rows: Vec<TreeRow>,
    rebuilds: usize,
    #[cfg(test)]
    rendered_rows: usize,
}

impl TreeRowProjectionCache {
    fn projection(
        &mut self,
        tree: &FolderTree,
        tree_generation: u64,
        state: &TreePanelState,
    ) -> &[TreeRow] {
        if self.tree_generation != Some(tree_generation)
            || self.expansion_generation != state.expansion_generation()
        {
            self.rows.clear();
            tree.flatten_visible(state, &mut self.rows);
            self.tree_generation = Some(tree_generation);
            self.expansion_generation = state.expansion_generation();
            self.rebuilds += 1;
        }
        &self.rows
    }

    #[cfg(test)]
    fn rebuild_count(&self) -> usize {
        self.rebuilds
    }

    #[cfg(test)]
    fn rendered_row_count(&self) -> usize {
        self.rendered_rows
    }
}

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut TreePanelState,
    archive_name: &str,
    tree: &FolderTree,
    tree_generation: u64,
    row_projection: &mut TreeRowProjectionCache,
    current_path: &str,
) -> Option<String> {
    let mut navigate_to: Option<String> = None;

    if state.set_selected_path(current_path) {
        state.auto_expand_current_path(current_path);
    }

    ui.add_space(4.0);

    // Header label
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new("ARCHIVE STRUCTURE")
                .size(12.0)
                .color(theme.colors.on_surface_variant)
                .strong(),
        );
    });

    ui.add_space(8.0);

    // Custom separator with theme color
    ui.horizontal(|ui| {
        let sep_rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(
            egui::Rect::from_min_size(sep_rect.min, egui::vec2(ui.available_width(), 1.0)),
            0.0,
            theme.colors.outline,
        );
        ui.allocate_space(egui::vec2(ui.available_width(), 1.0));
    });

    ui.add_space(8.0);

    #[cfg(test)]
    let mut rendered_rows = 0;
    {
        let rows = row_projection.projection(tree, tree_generation, state);
        egui::ScrollArea::vertical()
            .id_salt("tree_scroll")
            .auto_shrink([false; 2])
            .show_rows(ui, 32.0, rows.len(), |ui, row_range| {
                for row_index in row_range {
                    #[cfg(test)]
                    {
                        rendered_rows += 1;
                    }
                    match &rows[row_index] {
                        TreeRow::Root => {
                            if tree_item(
                                ui,
                                theme,
                                egui_phosphor::regular::PACKAGE,
                                archive_name,
                                archive_name,
                                0,
                                current_path.is_empty(),
                                false,
                            )
                            .clicked
                            {
                                navigate_to = Some(String::new());
                            }
                        }
                        TreeRow::Folder {
                            name,
                            full_path,
                            indent_level,
                            has_children,
                        } => {
                            let is_expanded = state.is_expanded(full_path);
                            let icon = if *has_children && is_expanded {
                                egui_phosphor::regular::FOLDER_OPEN
                            } else {
                                egui_phosphor::regular::FOLDER
                            };
                            let response = tree_item(
                                ui,
                                theme,
                                icon,
                                name,
                                full_path,
                                *indent_level,
                                current_path == full_path,
                                *has_children,
                            );
                            if response.clicked {
                                navigate_to = Some(full_path.clone());
                            }
                            if response.toggle_clicked && *has_children {
                                state.toggle_expanded(full_path);
                            }
                        }
                    }
                }
            });
    }
    #[cfg(test)]
    {
        row_projection.rendered_rows = rendered_rows;
    }

    navigate_to
}

struct TreeItemResponse {
    clicked: bool,
    toggle_clicked: bool,
}

fn tree_item(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    icon: &str,
    label: &str,
    id_source: &str,
    indent_level: usize,
    selected: bool,
    has_children: bool,
) -> TreeItemResponse {
    let indent = 16.0 + (indent_level as f32 * 16.0);
    let toggle_size = 16.0;

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());

    let mut toggle_clicked = false;

    if ui.is_rect_visible(rect) {
        // Set cursor to pointer when hovering
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Draw background for selected/hovered state
        let bg_color = if selected {
            // Use secondary for more visible selection
            theme.colors.secondary
        } else if response.hovered() {
            theme.colors.surface_variant
        } else {
            egui::Color32::TRANSPARENT
        };

        if bg_color != egui::Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, 4.0, bg_color);
        }

        // Draw expand/collapse triangle for folders with children
        if has_children && indent_level > 0 {
            let toggle_rect = egui::Rect::from_center_size(
                egui::pos2((rect.min.x + indent - 8.0).round(), rect.center().y.round()),
                egui::vec2(toggle_size, toggle_size),
            );

            let toggle_response = ui.interact(
                toggle_rect,
                ui.id().with(("toggle", id_source)),
                egui::Sense::click(),
            );
            if toggle_response.clicked() {
                toggle_clicked = true;
            }

            // Draw triangle (▶ or ▼) using phosphor icons
            let triangle_icon = if icon == egui_phosphor::regular::FOLDER_OPEN {
                egui_phosphor::regular::CARET_DOWN
            } else {
                egui_phosphor::regular::CARET_RIGHT
            };
            ui.painter().text(
                pixel_align(toggle_rect.center()),
                egui::Align2::CENTER_CENTER,
                triangle_icon,
                egui::FontId::proportional(10.0),
                theme.colors.on_surface_variant,
            );
        }

        // Draw icon and text separately. Joining them with `format!` used to
        // allocate a new String for every rendered row on every settled frame.
        let text_pos = pixel_align(egui::pos2(rect.min.x + indent, rect.center().y));

        // Use selection text color when selected, otherwise primary text color
        let text_color = if selected {
            theme.colors.on_surface
        } else {
            theme.colors.on_surface
        };

        ui.painter().text(
            text_pos,
            egui::Align2::LEFT_CENTER,
            icon,
            egui::FontId::proportional(14.0),
            text_color,
        );
        ui.painter().text(
            pixel_align(egui::pos2(text_pos.x + 18.0, text_pos.y)),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(14.0),
            text_color,
        );
    }

    TreeItemResponse {
        clicked: response.clicked(),
        toggle_clicked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(rows: &[TreeRow]) -> Vec<&str> {
        rows.iter().map(TreeRow::path_for_test).collect()
    }

    #[test]
    fn ten_thousand_tree_rows_render_only_the_small_viewport_slice() {
        let tree = FolderTree::from_folders(
            &(0..10_000)
                .map(|index| format!("folder-{index:05}"))
                .collect::<Vec<_>>(),
        );
        let mut state = TreePanelState::default();
        let mut rows = TreeRowProjectionCache::default();
        let theme = AppTheme::new(false);
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 320.0),
            )),
            ..Default::default()
        };

        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render(ui, &theme, &mut state, "large.zip", &tree, 1, &mut rows, "");
            });
        });

        assert!(
            rows.rendered_row_count() < 100,
            "show_rows visited {} of 10,001 rows",
            rows.rendered_row_count()
        );
    }

    #[test]
    fn settled_frames_reuse_the_flattened_row_allocation() {
        let tree = FolderTree::from_folders(&["a".to_string(), "b".to_string()]);
        let state = TreePanelState::default();
        let mut cache = TreeRowProjectionCache::default();

        let first_ptr = cache.projection(&tree, 5, &state).as_ptr();
        assert_eq!(cache.rebuild_count(), 1);
        let second_ptr = cache.projection(&tree, 5, &state).as_ptr();

        assert_eq!(first_ptr, second_ptr);
        assert_eq!(cache.rebuild_count(), 1);
    }

    #[test]
    fn toggling_parent_advances_one_generation_and_rebuilds_descendants() {
        let tree = FolderTree::from_folders(&["parent".to_string(), "parent/child".to_string()]);
        let mut state = TreePanelState::default();
        let mut cache = TreeRowProjectionCache::default();

        assert_eq!(paths(cache.projection(&tree, 1, &state)), ["", "parent"]);
        let generation = state.expansion_generation();

        assert!(state.toggle_expanded("parent"));
        assert_eq!(state.expansion_generation(), generation + 1);
        assert_eq!(
            paths(cache.projection(&tree, 1, &state)),
            ["", "parent", "parent/child"]
        );
        assert_eq!(cache.rebuild_count(), 2);
    }

    #[test]
    fn auto_expansion_invalidates_only_for_new_path_segments() {
        let tree =
            FolderTree::from_folders(&["a".to_string(), "a/b".to_string(), "a/b/c".to_string()]);
        let mut state = TreePanelState::default();
        let mut cache = TreeRowProjectionCache::default();
        cache.projection(&tree, 9, &state);

        assert!(state.auto_expand_current_path("a/b/c"));
        let expanded_generation = state.expansion_generation();
        assert_eq!(
            paths(cache.projection(&tree, 9, &state)),
            ["", "a", "a/b", "a/b/c"]
        );
        assert_eq!(cache.rebuild_count(), 2);

        assert!(!state.auto_expand_current_path("a/b/c"));
        assert_eq!(state.expansion_generation(), expanded_generation);
        cache.projection(&tree, 9, &state);
        assert_eq!(cache.rebuild_count(), 2);
    }

    #[test]
    fn tree_generation_invalidates_rows_when_folder_count_is_unchanged() {
        let first = FolderTree::from_folders(&["a".to_string()]);
        let second = FolderTree::from_folders(&["b".to_string()]);
        let state = TreePanelState::default();
        let mut cache = TreeRowProjectionCache::default();

        assert_eq!(paths(cache.projection(&first, 11, &state)), ["", "a"]);
        assert_eq!(paths(cache.projection(&second, 12, &state)), ["", "b"]);
        assert_eq!(cache.rebuild_count(), 2);
    }
}
