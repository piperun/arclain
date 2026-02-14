//! Standardized Panel Component
//!
//! A reusable panel with optional header, body sections, and footer.
//! Supports collapsible sections and theme-aware styling.

use crate::features::plugins::presentation::rendering as ui;

use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_plugins::manager::PluginManager;
use arclain_plugins::types::PluginUiElement;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

/// Header action types
#[derive(Clone)]
pub enum HeaderAction {
    #[allow(dead_code)]
    Button {
        id: String,
        label: String,
        on_click: Arc<dyn Fn() + Send + Sync>,
    },
    #[allow(dead_code)]
    Toggle {
        id: String,
        label: String,
        enabled: bool,
        on_toggle: Arc<dyn Fn(bool) + Send + Sync>,
    },
}

/// Panel header configuration
#[derive(Clone, Default)]
pub struct PanelHeader {
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: Option<String>,
    pub actions: Vec<HeaderAction>,
}

impl PanelHeader {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            icon: None,
            actions: Vec::new(),
        }
    }
}

/// Panel content types for the body
#[derive(Clone)]
pub enum PanelBody {
    /// Key-value property list
    Properties(Vec<(String, String)>),
    /// Plugin UI elements
    PluginUI {
        plugin_id: String,
        elements: Vec<PluginUiElement>,
    },
    /// Separator line
    #[allow(dead_code)]
    Separator,
    /// Space
    #[allow(dead_code)]
    Space(f32),
}

/// Action returned from panel interactions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelAction {
    None,
    HeaderAction(String),
    FooterAction(String),
}

/// Configuration for the Panel component
pub struct Panel {
    pub id: String,
    pub header: Option<PanelHeader>,
    pub body: Vec<PanelBody>,
    pub collapsible: bool,
    pub initially_collapsed: bool,
}

impl Panel {
    /// Create a new panel with an ID
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            header: None,
            body: Vec::new(),
            collapsible: false,
            initially_collapsed: false,
        }
    }

    /// Set the panel header
    pub fn with_header(mut self, header: PanelHeader) -> Self {
        self.header = Some(header);
        self
    }

    /// Add a body section
    pub fn with_body(mut self, body: PanelBody) -> Self {
        self.body.push(body);
        self
    }

    /// Make the panel collapsible
    #[allow(dead_code)]
    pub fn collapsible(mut self, initially_collapsed: bool) -> Self {
        self.collapsible = true;
        self.initially_collapsed = initially_collapsed;
        self
    }

    /// Render the panel and return any action triggered
    pub fn render(
        &self,
        ui: &mut egui::Ui,
        theme: &AppTheme,
        plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
        shared: Option<&SharedState>,
        content_cache: Option<&Arc<arclain_data::ContentCache>>,
    ) -> PanelAction {
        let mut action = PanelAction::None;

        let panel_frame = egui::Frame::NONE
            .fill(theme.colors.surface)
            .stroke(egui::Stroke::new(1.0, theme.colors.outline))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(0));

        panel_frame.show(ui, |ui| {
            ui.set_min_width(ui.available_width());

            if self.collapsible {
                // Use collapsible section
                let title = self
                    .header
                    .as_ref()
                    .map(|h| h.title.as_str())
                    .unwrap_or("Panel");

                arclain_widgets::CollapsibleSection::new(&self.id, title)
                    .with_theme_colors(&theme.colors)
                    .default_open(!self.initially_collapsed)
                    .show(ui, |ui| {
                        self.render_body(ui, theme, plugin_manager, shared, content_cache);
                    });
            } else {
                // Non-collapsible panel
                if let Some(header_action) = self.render_header(ui, theme) {
                    action = header_action;
                }

                ui.add_space(8.0);
                self.render_body(ui, theme, plugin_manager, shared, content_cache);
            }
        });

        action
    }

    fn render_header(&self, ui: &mut egui::Ui, theme: &AppTheme) -> Option<PanelAction> {
        let header = self.header.as_ref()?;
        let mut action = None;

        ui.horizontal(|ui| {
            ui.add_space(12.0);

            // Icon if present
            if let Some(icon) = &header.icon {
                ui.label(
                    egui::RichText::new(icon)
                        .size(14.0)
                        .color(theme.colors.on_surface_variant),
                );
                ui.add_space(4.0);
            }

            // Title
            ui.label(
                egui::RichText::new(&header.title)
                    .size(11.0)
                    .strong()
                    .color(theme.colors.on_surface_variant),
            );

            // Subtitle if present
            if let Some(subtitle) = &header.subtitle {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(subtitle)
                        .size(10.0)
                        .color(theme.colors.on_surface_variant),
                );
            }

            // Actions on the right
            if !header.actions.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(12.0);
                    for header_action in header.actions.iter().rev() {
                        match header_action {
                            HeaderAction::Button { id, label, .. } => {
                                if ui.small_button(label).clicked() {
                                    action = Some(PanelAction::HeaderAction(id.clone()));
                                }
                            }
                            HeaderAction::Toggle {
                                id, label, enabled, ..
                            } => {
                                let mut checked = *enabled;
                                if ui.checkbox(&mut checked, label).changed() {
                                    action = Some(PanelAction::HeaderAction(id.clone()));
                                }
                            }
                        }
                    }
                });
            }
        });

        action
    }

    fn render_body(
        &self,
        ui: &mut egui::Ui,
        theme: &AppTheme,
        plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
        shared: Option<&SharedState>,
        content_cache: Option<&Arc<arclain_data::ContentCache>>,
    ) {
        for body in &self.body {
            match body {
                PanelBody::Properties(props) => {
                    for (label, value) in props {
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);

                            // Fixed width for label column (approx 40% of panel width)
                            let label_width = (ui.available_width() * 0.4).min(120.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(label_width, ui.spacing().interact_size.y),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(label)
                                            .size(14.0)
                                            .color(theme.colors.on_surface_variant),
                                    );
                                },
                            );

                            // Value takes remaining space, right-aligned
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_space(12.0);
                                    // Truncate long values (char-based for UTF-8 safety)
                                    let display_value = if value.chars().count() > 25 {
                                        let truncated: String = value.chars().take(22).collect();
                                        format!("{}...", truncated)
                                    } else {
                                        value.clone()
                                    };
                                    ui.label(
                                        egui::RichText::new(display_value)
                                            .size(14.0)
                                            .strong()
                                            .color(theme.colors.on_surface),
                                    );
                                },
                            );
                        });
                        ui.add_space(4.0);
                    }
                }
                PanelBody::PluginUI {
                    plugin_id,
                    elements,
                } => {
                    // content_cache is now passed as parameter (extracted by caller before rendering)

                    if let Some(manager_arc) = plugin_manager {
                        // Get plugin instance handle
                        let instance_arc = {
                            let manager = manager_arc.lock();
                            manager.get_plugin_instance(plugin_id)
                        };

                        if let Some(ref instance_arc) = instance_arc {
                            let instance_arc = instance_arc.clone();
                            let pid = plugin_id.clone();

                            // Get dialog state for navigation
                            let dialog_signal =
                                shared.map(|s| s.signals().plugin_dialog_state.clone());

                            let mut callback: ui::UiEventCallback =
                                Box::new(move |element_id: &str, value: Option<String>| {
                                    // Handle page navigation
                                    if element_id.starts_with("__page_open:") {
                                        if let Some(ref signal) = dialog_signal {
                                            let page_id =
                                                element_id.trim_start_matches("__page_open:");
                                            let mut ds = signal.get();
                                            ds.open_page(&pid, page_id);
                                            signal.set(ds);
                                        }
                                        return;
                                    }

                                    // Handle dialog open
                                    if element_id.starts_with("__dialog_open:") {
                                        if let Some(ref signal) = dialog_signal {
                                            let dialog_id =
                                                element_id.trim_start_matches("__dialog_open:");
                                            let mut ds = signal.get();
                                            ds.open_dialog(&pid, dialog_id);
                                            signal.set(ds);
                                        }
                                        return;
                                    }

                                    // Normal event - send to plugin
                                    let mut instance = instance_arc.lock();
                                    let _ = instance.send_ui_event(element_id, value);
                                });

                            ui::render_ui_elements(
                                ui,
                                elements,
                                &mut callback,
                                &theme.colors,
                                content_cache,
                                shared,
                                Some(plugin_id.as_str()),
                            );
                        }
                    } else {
                        // No plugin manager - just render elements without event handling
                        let mut callback: ui::UiEventCallback =
                            Box::new(|_id: &str, _val: Option<String>| {});

                        ui::render_ui_elements(
                            ui,
                            elements,
                            &mut callback,
                            &theme.colors,
                            content_cache,
                            shared,
                            None,
                        );
                    }
                }
                PanelBody::Separator => {
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                }
                PanelBody::Space(amount) => {
                    ui.add_space(*amount);
                }
            }
        }
    }
}
