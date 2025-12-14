use crate::features::plugins::plugin_ui;
use crate::shared::theme::AppTheme;
use arclain_plugins::manager::PluginManager;
use arclain_plugins::types::PluginExtensionPoint;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

pub struct PropertyGroup {
    pub title: String,
    pub properties: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertiesPanelAction {
    None,
    #[allow(dead_code)] // Used in pattern matching but never constructed yet
    Organize,
    Metadata(String),
}

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    groups: &[PropertyGroup],
    plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
) -> PropertiesPanelAction {
    let mut action = PropertiesPanelAction::None;

    ui.vertical(|ui| {
        ui.add_space(4.0);

        for (idx, group) in groups.iter().enumerate() {
            if idx > 0 {
                ui.add_space(8.0);
            }

            render_property_group(ui, theme, group);
        }

        // Render Plugin Sidebar UI
        if let Some(manager_arc) = plugin_manager {
            let manager = manager_arc.lock();

            let plugins: Vec<String> = manager
                .list_plugins()
                .iter()
                .filter(|p| p.enabled)
                .map(|p| p.id.clone())
                .collect();

            for plugin_id in plugins {
                let metadata_opt = manager.with_plugin_instance(&plugin_id, |instance| {
                    if let Ok(ui_elements) = instance.get_ui_layout(PluginExtensionPoint::Sidebar) {
                        // Check for pending messages
                        let messages = instance.get_pending_messages();
                        for (title, message) in messages {
                            tracing::info!("PLUGIN MESSAGE: {} - {}", title, message);
                        }

                        if !ui_elements.is_empty() {
                            ui.add_space(8.0);

                            let id = ui.make_persistent_id(&plugin_id);
                            let plugin_name = instance
                                .get_metadata()
                                .map(|m| m.name)
                                .unwrap_or_else(|_| "Unknown".to_string());

                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                ui.ctx(),
                                id,
                                true,
                            )
                            .show_header(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(format!("{} Plugin", plugin_name))
                                        .size(11.0)
                                        .strong()
                                        .color(theme.colors.on_surface_variant),
                                );
                            })
                            .body(|ui| {
                                ui.add_space(4.0);
                                ui.add_space(4.0);

                                let mut callback: Box<dyn FnMut(&str, Option<String>)> =
                                    Box::new(|element_id: &str, value: Option<String>| {
                                        tracing::info!("UI Event: {} = {:?}", element_id, value);
                                        let _ = instance.send_ui_event(element_id, value);
                                    });

                                plugin_ui::render_ui_elements(ui, &ui_elements, &mut callback);
                            });
                        }
                    }

                    // Check for emitted metadata
                    instance.get_emitted_metadata()
                });

                if let Some(Some(metadata)) = metadata_opt {
                    action = PropertiesPanelAction::Metadata(metadata);
                }
            }
        }
    });

    action
}

fn render_property_group(ui: &mut egui::Ui, theme: &AppTheme, group: &PropertyGroup) {
    let group_frame = egui::Frame::NONE
        .fill(theme.colors.surface)
        .stroke(egui::Stroke::new(1.0, theme.colors.outline))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(0, 12));

    group_frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());

        // Group title
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(&group.title)
                    .size(11.0)
                    .strong()
                    .color(theme.colors.on_surface_variant),
            );
        });

        ui.add_space(8.0);

        // Properties
        for (label, value) in &group.properties {
            ui.horizontal(|ui| {
                ui.add_space(12.0);

                ui.label(
                    egui::RichText::new(label)
                        .size(14.0)
                        .color(theme.colors.on_surface_variant),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new(value)
                            .size(14.0)
                            .strong()
                            .color(theme.colors.on_surface),
                    );
                });
            });

            ui.add_space(4.0);
        }
    });
}

pub fn create_file_info_group(
    name: &str,
    size: &str,
    compressed: &str,
    ratio: &str,
) -> PropertyGroup {
    PropertyGroup {
        title: "FILE INFORMATION".to_string(),
        properties: vec![
            ("Name:".to_string(), name.to_string()),
            ("Size:".to_string(), size.to_string()),
            ("Compressed:".to_string(), compressed.to_string()),
            ("Ratio:".to_string(), ratio.to_string()),
        ],
    }
}

pub fn create_attributes_group(modified: &str, crc32: &str, method: &str) -> PropertyGroup {
    PropertyGroup {
        title: "ATTRIBUTES".to_string(),
        properties: vec![
            ("Modified:".to_string(), modified.to_string()),
            ("CRC32:".to_string(), crc32.to_string()),
            ("Method:".to_string(), method.to_string()),
        ],
    }
}

pub fn create_archive_info_group(
    format: &str,
    total_files: usize,
    total_size: &str,
    compressed_size: &str,
    total_crc32: Option<&str>,
    encrypted: bool,
    headers_encrypted: bool,
    encryption_method: Option<&str>,
) -> PropertyGroup {
    // Data vs header encryption clarity
    let (data_enc_label, method_line): (String, Option<String>) = if encrypted {
        if let Some(method) = encryption_method {
            ("Yes".to_string(), Some(method.to_string()))
        } else {
            ("Yes".to_string(), None)
        }
    } else {
        ("No".to_string(), None)
    };

    let header_status = if headers_encrypted { "Yes" } else { "No" };
    let tcrc_display = total_crc32.unwrap_or("—");

    let mut props = vec![
        ("Total Files:".to_string(), total_files.to_string()),
        ("Total Size:".to_string(), total_size.to_string()),
        ("Compressed:".to_string(), compressed_size.to_string()),
        ("Total CRC-32:".to_string(), tcrc_display.to_string()),
        ("Format:".to_string(), format.to_string()),
        ("Data Encrypted:".to_string(), data_enc_label),
    ];
    if let Some(detail) = method_line {
        props.push(("".to_string(), detail));
    }
    props.push(("Headers Encrypted:".to_string(), header_status.to_string()));

    PropertyGroup {
        title: "ARCHIVE INFO".to_string(),
        properties: props,
    }
}

/// Create a property group for plugin metadata
pub fn create_plugin_metadata_group(metadata: &serde_json::Value) -> Option<PropertyGroup> {
    if !metadata.is_object() {
        return None;
    }

    let obj = metadata.as_object()?;
    if obj.is_empty() {
        return None;
    }

    let mut properties = Vec::new();

    // Extract common fields with nice formatting
    for (key, value) in obj.iter() {
        let display_value = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            _ => continue,
        };

        // Format key to be more readable
        let display_key = key
            .split('_')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        properties.push((format!("{}:", display_key), display_value));
    }

    if properties.is_empty() {
        return None;
    }

    Some(PropertyGroup {
        title: "PLUGIN METADATA".to_string(),
        properties,
    })
}
