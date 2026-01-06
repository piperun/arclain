//! Context menu component for file list right-click actions.
//!
//! NOTE: This module provides DB-driven context menu rendering for future use.
//! Currently, context menus are rendered inline in file_list.rs.

#![allow(dead_code)]

use arclain_core::{DisplayMode, UiItem, UiRegion};
use arclain_theme::AppTheme;
use eframe::egui;

/// Actions that can be triggered from the context menu
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuAction {
    None,
    Open,
    Extract,
    ExtractTo,
    CopyPath,
    Delete,
    Properties,
    Custom(String),
}

impl ContextMenuAction {
    /// Convert item ID to action
    pub fn from_id(id: &str) -> Self {
        match id {
            "context.open" => ContextMenuAction::Open,
            "context.extract" => ContextMenuAction::Extract,
            "context.extract_to" => ContextMenuAction::ExtractTo,
            "context.copy_path" => ContextMenuAction::CopyPath,
            "context.delete" => ContextMenuAction::Delete,
            "context.properties" => ContextMenuAction::Properties,
            _ => ContextMenuAction::Custom(id.to_string()),
        }
    }
}

/// Render file context menu using configured items
pub fn render_file_context_menu(
    ui: &mut egui::Ui,
    _theme: &AppTheme,
    items: &[UiItem],
    has_selection: bool,
) -> ContextMenuAction {
    let mut action = ContextMenuAction::None;

    // Filter visible context menu items and sort by order
    let mut context_items: Vec<_> = items
        .iter()
        .filter(|i| i.region == UiRegion::ContextMenu && i.visible)
        .collect();
    context_items.sort_by_key(|i| i.sort_order);

    for item in context_items {
        let enabled = match item.id.as_str() {
            "context.extract" | "context.extract_to" | "context.delete" | "context.properties" => {
                has_selection
            }
            _ => true,
        };

        let button = match item.display_mode {
            DisplayMode::IconOnly => {
                let icon = icon_name_to_char(item.icon.as_deref().unwrap_or("QUESTION"));
                egui::Button::new(egui::RichText::new(icon).size(14.0))
            }
            DisplayMode::TextOnly => egui::Button::new(egui::RichText::new(&item.label).size(13.0)),
            DisplayMode::IconAndText => {
                let icon = icon_name_to_char(item.icon.as_deref().unwrap_or("QUESTION"));
                egui::Button::new(
                    egui::RichText::new(format!("{} {}", icon, item.label)).size(13.0),
                )
            }
        };

        let button = button
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
            .min_size(egui::vec2(120.0, 24.0));

        if ui
            .add_enabled(enabled, button)
            .on_hover_text(&item.label)
            .clicked()
        {
            action = ContextMenuAction::from_id(&item.id);
            ui.close();
        }
    }

    action
}

/// Map icon name to Phosphor icon character
fn icon_name_to_char(name: &str) -> &'static str {
    match name {
        "FOLDER_OPEN" => egui_phosphor::regular::FOLDER_OPEN,
        "EXPORT" => egui_phosphor::regular::EXPORT,
        "COPY" => egui_phosphor::regular::COPY,
        "TRASH" => egui_phosphor::regular::TRASH,
        "INFO" => egui_phosphor::regular::INFO,
        _ => egui_phosphor::regular::QUESTION,
    }
}
