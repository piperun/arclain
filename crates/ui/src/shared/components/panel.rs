//! Standardized Panel Component
//!
//! A reusable panel with optional header, body sections, and footer.
//! Supports collapsible sections and theme-aware styling.

use crate::features::plugins::plugin_ui;
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
    Button {
        id: String,
        label: String,
        icon: Option<String>,
    },
    Toggle {
        id: String,
        label: String,
        enabled: bool,
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

    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
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
    Separator,
    /// Space
    Space(f32),
}

/// Footer button configuration
#[derive(Clone)]
pub struct FooterButton {
    pub id: String,
    pub label: String,
    pub primary: bool,
}

/// Panel footer configuration
#[derive(Clone, Default)]
pub struct PanelFooter {
    pub buttons: Vec<FooterButton>,
}

impl PanelFooter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_button(
        mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        primary: bool,
    ) -> Self {
        self.buttons.push(FooterButton {
            id: id.into(),
            label: label.into(),
            primary,
        });
        self
    }
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
    pub footer: Option<PanelFooter>,
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
            footer: None,
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

    /// Add multiple body sections
    pub fn with_bodies(mut self, bodies: Vec<PanelBody>) -> Self {
        self.body.extend(bodies);
        self
    }

    /// Set the panel footer
    pub fn with_footer(mut self, footer: PanelFooter) -> Self {
        self.footer = Some(footer);
        self
    }

    /// Make the panel collapsible
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
                        self.render_body(ui, theme, plugin_manager, shared);
                        if let Some(footer_action) = self.render_footer(ui, theme) {
                            action = footer_action;
                        }
                    });
            } else {
                // Non-collapsible panel
                if let Some(header_action) = self.render_header(ui, theme) {
                    action = header_action;
                }

                ui.add_space(8.0);
                self.render_body(ui, theme, plugin_manager, shared);

                if let Some(footer_action) = self.render_footer(ui, theme) {
                    action = footer_action;
                }
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
                            HeaderAction::Toggle { id, label, enabled } => {
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
    ) {
        for body in &self.body {
            match body {
                PanelBody::Properties(props) => {
                    for (label, value) in props {
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.label(
                                egui::RichText::new(label)
                                    .size(14.0)
                                    .color(theme.colors.on_surface_variant),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_space(12.0);
                                    ui.label(
                                        egui::RichText::new(value)
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
                    if let Some(manager_arc) = plugin_manager {
                        // Get plugin instance handle
                        let instance_arc = {
                            let manager = manager_arc.lock();
                            manager.get_plugin_instance(plugin_id)
                        };

                        if let Some(ref instance_arc) = instance_arc {
                            let instance_arc = instance_arc.clone();
                            let pid = plugin_id.clone();
                            let mut callback: plugin_ui::UiEventCallback =
                                Box::new(move |element_id: &str, value: Option<String>| {
                                    let mut instance = instance_arc.lock();
                                    let _ = instance.send_ui_event(element_id, value);
                                });

                            plugin_ui::render_ui_elements(
                                ui,
                                elements,
                                &mut callback,
                                &theme.colors,
                                None,
                                shared,
                                Some(plugin_id.as_str()),
                            );
                        }
                    } else {
                        // No plugin manager - just render elements without event handling
                        let mut callback: plugin_ui::UiEventCallback =
                            Box::new(|_id: &str, _val: Option<String>| {});

                        plugin_ui::render_ui_elements(
                            ui,
                            elements,
                            &mut callback,
                            &theme.colors,
                            None,
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

    fn render_footer(&self, ui: &mut egui::Ui, theme: &AppTheme) -> Option<PanelAction> {
        let footer = self.footer.as_ref()?;
        if footer.buttons.is_empty() {
            return None;
        }

        let mut action = None;

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.add_space(12.0);
            for btn in &footer.buttons {
                let button = if btn.primary {
                    egui::Button::new(
                        egui::RichText::new(&btn.label).color(theme.colors.on_primary),
                    )
                    .fill(theme.colors.primary)
                } else {
                    egui::Button::new(&btn.label)
                };

                if ui.add(button).clicked() {
                    action = Some(PanelAction::FooterAction(btn.id.clone()));
                }
            }
        });

        ui.add_space(8.0);

        action
    }
}

/// Builder for creating panels easily
pub struct PanelBuilder {
    panel: Panel,
}

impl PanelBuilder {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            panel: Panel::new(id),
        }
    }

    pub fn header(mut self, title: impl Into<String>) -> Self {
        self.panel.header = Some(PanelHeader::new(title));
        self
    }

    pub fn properties(mut self, props: Vec<(impl Into<String>, impl Into<String>)>) -> Self {
        let props: Vec<(String, String)> = props
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self.panel.body.push(PanelBody::Properties(props));
        self
    }

    pub fn plugin_ui(
        mut self,
        plugin_id: impl Into<String>,
        elements: Vec<PluginUiElement>,
    ) -> Self {
        self.panel.body.push(PanelBody::PluginUI {
            plugin_id: plugin_id.into(),
            elements,
        });
        self
    }

    pub fn separator(mut self) -> Self {
        self.panel.body.push(PanelBody::Separator);
        self
    }

    pub fn space(mut self, amount: f32) -> Self {
        self.panel.body.push(PanelBody::Space(amount));
        self
    }

    pub fn collapsible(mut self, initially_collapsed: bool) -> Self {
        self.panel.collapsible = true;
        self.panel.initially_collapsed = initially_collapsed;
        self
    }

    pub fn build(self) -> Panel {
        self.panel
    }
}
