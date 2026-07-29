//! Properties Panel Component
//!
//! Renders the properties panel using the standardized Panel component.

use super::panel::{Panel, PanelBody, PanelHeader};
use crate::features::plugins::application::PluginSlot;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_app::plugins::PluginUiDocument;
use eframe::egui;
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
        slot: PluginSlot,
        document: Arc<PluginUiDocument>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertiesPanelAction {
    None,
}

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    sections: &[PanelSection],
    shared: Option<&SharedState>,
) -> PropertiesPanelAction {
    let action = PropertiesPanelAction::None;

    // Extract content_cache from services (no lock needed)
    let content_cache = shared.and_then(|s| s.services.content_cache.clone());

    ui.vertical(|ui| {
        for (idx, section) in sections.iter().enumerate() {
            if idx > 0 {
                ui.add_space(4.0);
            }

            match section {
                PanelSection::Group(group) => {
                    // Use standardized Panel for property groups
                    let panel = Panel::new(format!("group_{}", idx))
                        .with_header(PanelHeader::new(&group.title))
                        .with_body(PanelBody::Properties(group.properties.clone()));

                    panel.render(ui, theme, shared, content_cache.as_ref());
                }
                PanelSection::Plugin { slot, document } => {
                    let plugin_id = slot.plugin_id();
                    let plugin_name = shared
                        .and_then(|shared| {
                            shared
                                .plugin_ui_jobs
                                .plugin_snapshot(shared.signals().plugin_visibility.get())
                        })
                        .and_then(Result::ok)
                        .and_then(|plugins| {
                            plugins
                                .iter()
                                .find(|plugin| plugin.id == plugin_id)
                                .map(|plugin| plugin.name.clone())
                        })
                        .unwrap_or_else(|| "Plugin".to_string());

                    let panel = Panel::new(format!("plugin_{plugin_id}"))
                        .with_header(PanelHeader::new(plugin_name))
                        .with_body(PanelBody::PluginUI {
                            slot: slot.clone(),
                            document: document.clone(),
                        });

                    panel.render(ui, theme, shared, content_cache.as_ref());
                }
            }
        }
    });

    action
}

// Helper functions for creating property groups

pub fn create_file_info_group(
    name: &str,
    size: &str,
    compressed: &str,
    ratio: &str,
) -> PropertyGroup {
    PropertyGroup {
        title: "FILE INFORMATION".to_string(),
        properties: vec![
            ("Name".to_string(), name.to_string()),
            ("Size".to_string(), size.to_string()),
            ("Compressed".to_string(), compressed.to_string()),
            ("Ratio".to_string(), ratio.to_string()),
        ],
    }
}

pub fn create_attributes_group(modified: &str, crc32: &str, method: &str) -> PropertyGroup {
    PropertyGroup {
        title: "ATTRIBUTES".to_string(),
        properties: vec![
            ("Modified".to_string(), modified.to_string()),
            ("CRC32".to_string(), crc32.to_string()),
            ("Method".to_string(), method.to_string()),
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
    let data_enc = if encrypted { "Yes" } else { "No" };
    let header_enc = if headers_encrypted { "Yes" } else { "No" };
    let crc_display = total_crc32.unwrap_or("—");

    let mut props = vec![
        ("Total Files".to_string(), total_files.to_string()),
        ("Total Size".to_string(), total_size.to_string()),
        ("Compressed".to_string(), compressed_size.to_string()),
        ("Total CRC-32".to_string(), crc_display.to_string()),
        ("Format".to_string(), format.to_string()),
        ("Data Encrypted".to_string(), data_enc.to_string()),
    ];

    if encrypted {
        if let Some(method) = encryption_method {
            props.push(("Encryption".to_string(), method.to_string()));
        }
    }

    props.push(("Headers Encrypted".to_string(), header_enc.to_string()));

    PropertyGroup {
        title: "ARCHIVE INFO".to_string(),
        properties: props,
    }
}

pub fn create_plugin_metadata_group(metadata: &serde_json::Value) -> Option<PropertyGroup> {
    let obj = metadata.as_object()?;
    if obj.is_empty() {
        return None;
    }

    let mut properties = Vec::new();

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

        // Format key nicely
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

        properties.push((display_key, display_value));
    }

    if properties.is_empty() {
        return None;
    }

    Some(PropertyGroup {
        title: "PLUGIN METADATA".to_string(),
        properties,
    })
}
