use crate::shared::theme::AppTheme;
use arclain_widgets::pixel_align;
use eframe::egui;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct TreePanelState {
    pub selected_path: String,
    pub expanded_folders: HashMap<String, bool>,
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

fn build_tree_structure(folders: &[String]) -> Vec<TreeNode> {
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
    root_nodes
}

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut TreePanelState,
    archive_name: &str,
    folders: &[String],
    current_path: &str,
) -> Option<String> {
    let mut navigate_to: Option<String> = None;

    state.selected_path = current_path.to_string();

    if !current_path.is_empty() {
        let mut path_accumulator = String::new();
        for segment in current_path.split('/') {
            if !path_accumulator.is_empty() {
                path_accumulator.push('/');
            }
            path_accumulator.push_str(segment);
            state
                .expanded_folders
                .entry(path_accumulator.clone())
                .or_insert(true);
        }
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

    // Build tree structure
    let tree = build_tree_structure(folders);

    // Tree view with egui_ltreeview
    egui::ScrollArea::vertical()
        .id_salt("tree_scroll")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.add_space(4.0);

            // Root archive item
            let is_root_selected = current_path.is_empty();
            if tree_item(
                ui,
                theme,
                egui_phosphor::regular::PACKAGE,
                archive_name,
                archive_name,
                0,
                is_root_selected,
                false,
            )
            .clicked
            {
                navigate_to = Some(String::new());
            }

            // Render tree nodes recursively
            for node in &tree {
                if let Some(path) = render_tree_node(ui, theme, state, node, current_path) {
                    navigate_to = Some(path);
                }
            }

            ui.add_space(4.0);
        });

    navigate_to
}

fn render_tree_node(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut TreePanelState,
    node: &TreeNode,
    current_path: &str,
) -> Option<String> {
    let mut navigate_to: Option<String> = None;

    let is_expanded = state
        .expanded_folders
        .get(&node.full_path)
        .copied()
        .unwrap_or(false);
    let has_children = !node.children.is_empty();
    let is_selected = current_path == node.full_path;

    let icon = if has_children {
        if is_expanded {
            egui_phosphor::regular::FOLDER_OPEN
        } else {
            egui_phosphor::regular::FOLDER
        }
    } else {
        egui_phosphor::regular::FOLDER
    };

    let response = tree_item(
        ui,
        theme,
        icon,
        &node.name,
        &node.full_path,
        node.indent_level,
        is_selected,
        has_children,
    );

    if response.clicked {
        navigate_to = Some(node.full_path.clone());
    }

    if response.toggle_clicked && has_children {
        state
            .expanded_folders
            .insert(node.full_path.clone(), !is_expanded);
    }

    // Render children if expanded
    if is_expanded && has_children {
        for child in &node.children {
            if let Some(path) = render_tree_node(ui, theme, state, child, current_path) {
                navigate_to = Some(path);
            }
        }
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

        // Draw icon and text
        let text_pos = pixel_align(egui::pos2(rect.min.x + indent, rect.center().y));
        let text = format!("{} {}", icon, label);

        // Use selection text color when selected, otherwise primary text color
        let text_color = if selected {
            theme.colors.on_surface
        } else {
            theme.colors.on_surface
        };

        ui.painter().text(
            text_pos,
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(14.0),
            text_color,
        );
    }

    TreeItemResponse {
        clicked: response.clicked(),
        toggle_clicked,
    }
}
