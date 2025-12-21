use crate::features::plugins::plugin_ui;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_plugins::manager::PluginManager;
use arclain_plugins::types::PluginUiElement;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyGroup {
    pub title: String,
    pub properties: Vec<(String, String)>,
}

#[derive(Clone)]
pub enum PanelSection {
    Group(PropertyGroup),
    Plugin {
        plugin_id: String,
        elements: Vec<PluginUiElement>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertiesPanelAction {
    None,
    #[allow(dead_code)] // Used in pattern matching but never constructed yet
    Organize,
    #[allow(dead_code)] // Future use for metadata from plugin actions
    Metadata(String),
}

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    sections: &[PanelSection],
    plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
    shared: Option<&SharedState>,
) -> PropertiesPanelAction {
    let action = PropertiesPanelAction::None;

    // Collect actions from plugin UI events to process after rendering
    let collected_actions: Arc<Mutex<Vec<(String, arclain_plugins::types::PluginAction)>>> =
        Arc::new(Mutex::new(Vec::new()));

    // Also collect dialog control signals
    let dialog_signals: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

    // Collect page navigation signals
    let page_signals: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

    ui.vertical(|ui| {
        ui.add_space(4.0);

        for (idx, section) in sections.iter().enumerate() {
            if idx > 0 {
                ui.add_space(8.0);
            }

            match section {
                PanelSection::Group(group) => {
                    render_property_group(ui, theme, group);
                }
                PanelSection::Plugin {
                    plugin_id,
                    elements,
                } => {
                    if let Some(manager_arc) = plugin_manager {
                        // Get plugin instance handle BEFORE creating callback to avoid nested locks
                        let instance_arc = {
                            let manager = manager_arc.lock();
                            manager.get_plugin_instance(plugin_id)
                        };

                        // Get plugin name for header (separate lock scope)
                        let plugin_name = {
                            let manager = manager_arc.lock();
                            manager
                                .with_plugin_instance(plugin_id, |instance| {
                                    instance
                                        .get_metadata()
                                        .map(|m| m.name)
                                        .unwrap_or_else(|_| "Unknown".to_string())
                                })
                                .unwrap_or_else(|| "Unknown".to_string())
                        };

                        arclain_widgets::CollapsibleSection::new(
                            &format!("{}_info", plugin_id),
                            &format!("{} Info", plugin_name),
                        )
                        .with_theme_colors(&theme.colors)
                        .show(ui, |ui| {
                            ui.add_space(4.0);

                            // Only render if we have the instance
                            if let Some(ref instance_arc) = instance_arc {
                                let instance_arc = instance_arc.clone();
                                let pid = plugin_id.clone();
                                let actions_sink = collected_actions.clone();
                                let dialog_sink = dialog_signals.clone();
                                let page_sink = page_signals.clone();
                                let mut callback: Box<dyn FnMut(&str, Option<String>)> =
                                    Box::new(move |element_id: &str, value: Option<String>| {
                                        // Check for dialog control signals
                                        if element_id.starts_with("__dialog_open:") {
                                            let dialog_id = element_id
                                                .trim_start_matches("__dialog_open:")
                                                .to_string();
                                            dialog_sink.lock().push((pid.clone(), dialog_id));
                                            return;
                                        }
                                        if element_id == "__dialog_close" {
                                            dialog_sink
                                                .lock()
                                                .push((pid.clone(), "__close".to_string()));
                                            return;
                                        }
                                        if element_id.starts_with("__page_open:") {
                                            let page_id = element_id
                                                .trim_start_matches("__page_open:")
                                                .to_string();
                                            page_sink.lock().push((pid.clone(), page_id));
                                            return;
                                        }
                                        if element_id == "__page_close" {
                                            page_sink
                                                .lock()
                                                .push((pid.clone(), "__close".to_string()));
                                            return;
                                        }

                                        // Normal event - send to plugin instance directly (no manager lock!)
                                        let mut instance = instance_arc.lock();
                                        if let Ok(actions) =
                                            instance.send_ui_event(element_id, value)
                                        {
                                            let mut sink = actions_sink.lock();
                                            for a in actions {
                                                sink.push((pid.clone(), a));
                                            }
                                        }
                                    });

                                plugin_ui::render_ui_elements(
                                    ui,
                                    elements,
                                    &mut callback,
                                    &theme.colors,
                                    None, // TODO: wire content_cache through
                                );
                            }
                        });
                    }
                }
            }
        }
    });

    // Process collected plugin actions
    if let Some(shared) = shared {
        let actions = collected_actions.lock();
        let mut toaster = shared.toaster.lock();
        let mut dialog_state = shared.plugin_dialog_state.lock();

        for (plugin_id, plugin_action) in actions.iter() {
            crate::features::plugins::action_handler::process_plugin_actions(
                vec![plugin_action.clone()],
                plugin_id,
                &mut dialog_state,
                &mut toaster,
                Some(&shared.refresh_requests),
            );
        }

        // Process dialog control signals
        let signals = dialog_signals.lock();
        for (plugin_id, dialog_id) in signals.iter() {
            if dialog_id == "__close" {
                dialog_state.close_dialog();
            } else {
                dialog_state.open_dialog(plugin_id, dialog_id);
            }
        }

        // Process page navigation signals
        let page_sigs = page_signals.lock();
        for (plugin_id, page_id) in page_sigs.iter() {
            if page_id == "__close" {
                dialog_state.close_page();
            } else {
                dialog_state.open_page(plugin_id, page_id);
            }
        }
    }

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
