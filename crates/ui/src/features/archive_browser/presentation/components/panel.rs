//! Standardized Panel Component
//!
//! A reusable panel with optional header and body sections, theme-aware.
//! The collapsible/header-action/footer-action machinery from the
//! original sketch was never wired up, so it's been trimmed here.

use crate::features::plugins::application::PluginSlot;
use crate::features::plugins::presentation::document_dispatch;
use crate::features::plugins::presentation::rendering::{render_document, DocumentContext};

use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_app::plugins::PluginUiDocument;
use eframe::egui;
use std::sync::Arc;

/// Panel header configuration
#[derive(Clone, Default)]
pub struct PanelHeader {
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: Option<String>,
}

impl PanelHeader {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            icon: None,
        }
    }
}

/// Panel content types for the body
#[derive(Clone)]
pub enum PanelBody {
    /// Key-value property list
    Properties(Vec<(String, String)>),
    /// A plugin's `Panel` document, served by the application facade's
    /// session contract -- see
    /// `crate::features::plugins::application::facade_sessions`. The
    /// caller resolves the slot and its document; this component only
    /// draws it and routes what the user did back through the shared
    /// document dispatcher.
    PluginUI {
        slot: PluginSlot,
        document: Arc<PluginUiDocument>,
    },
}

/// Configuration for the Panel component
pub struct Panel {
    pub id: String,
    pub header: Option<PanelHeader>,
    pub body: Vec<PanelBody>,
}

impl Panel {
    /// Create a new panel with an ID
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            header: None,
            body: Vec::new(),
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

    /// Render the panel
    pub fn render(
        &self,
        ui: &mut egui::Ui,
        theme: &AppTheme,
        shared: Option<&SharedState>,
        content_cache: Option<&Arc<arclain_core::ContentCache>>,
    ) {
        let panel_frame = egui::Frame::NONE
            .fill(theme.colors.surface)
            .stroke(egui::Stroke::new(1.0, theme.colors.outline))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(0));

        panel_frame.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            self.render_header(ui, theme);
            ui.add_space(8.0);
            self.render_body(ui, theme, shared, content_cache);
        });
    }

    fn render_header(&self, ui: &mut egui::Ui, theme: &AppTheme) {
        let Some(header) = self.header.as_ref() else {
            return;
        };

        ui.horizontal(|ui| {
            ui.add_space(12.0);

            if let Some(icon) = &header.icon {
                ui.label(
                    egui::RichText::new(icon)
                        .size(14.0)
                        .color(theme.colors.on_surface_variant),
                );
                ui.add_space(4.0);
            }

            ui.label(
                egui::RichText::new(&header.title)
                    .size(11.0)
                    .strong()
                    .color(theme.colors.on_surface_variant),
            );

            if let Some(subtitle) = &header.subtitle {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(subtitle)
                        .size(10.0)
                        .color(theme.colors.on_surface_variant),
                );
            }
        });
    }

    fn render_body(
        &self,
        ui: &mut egui::Ui,
        theme: &AppTheme,
        shared: Option<&SharedState>,
        _content_cache: Option<&Arc<arclain_core::ContentCache>>,
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
                PanelBody::PluginUI { slot, document } => {
                    // A `Panel` slot is always tab-scoped, so this cannot
                    // fall back to the active tab -- routing an event from
                    // an inactive tab's panel to whichever tab happens to
                    // be active is exactly the bug per-tab slots exist to
                    // prevent.
                    let Some(origin_tab) = slot.tab() else {
                        continue;
                    };
                    let image_owner = crate::shared::image_assets::ImageOwner::plugin_panel(
                        slot.plugin_id(),
                        &self.id,
                        origin_tab,
                    );
                    let events = render_document(
                        ui,
                        document,
                        DocumentContext {
                            colors: &theme.colors,
                            shared_state: shared,
                            image_owner: Some(&image_owner),
                        },
                    );
                    if let Some(shared) = shared {
                        document_dispatch::apply_document_events(shared, slot, origin_tab, events);
                    }
                }
            }
        }
    }
}
