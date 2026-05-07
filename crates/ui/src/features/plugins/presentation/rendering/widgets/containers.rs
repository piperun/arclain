//! Structural / non-input widgets — group other widgets, don't take
//! direct user input themselves (tabs and toolbar emit events but
//! they're navigation/dispatch, not value changes).

use super::super::context::{RenderContext, UiEventHandler};
use crate::shared::components::settings_form::{SectionHeader, SettingsGroup};
use arclain_plugins::types::PluginUiElement;
use eframe::egui;

pub fn render_tabs(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    tabs: &[String],
    selected: &str,
) {
    let colors = ctx.colors;

    // Pill-style container
    egui::Frame::NONE
        .fill(colors.surface_variant)
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(4, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;

                for tab in tabs {
                    let is_selected = tab == selected;

                    // Tab button styling
                    let (bg_color, text_color) = if is_selected {
                        (colors.primary, colors.on_primary)
                    } else {
                        (egui::Color32::TRANSPARENT, colors.on_surface_variant)
                    };

                    let button = egui::Button::new(
                        egui::RichText::new(tab).size(13.0).color(text_color),
                    )
                    .fill(bg_color)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(6.0)
                    .min_size(egui::vec2(0.0, 28.0));

                    let response = ui.add(button);

                    // Hover effect for non-selected tabs
                    if !is_selected && response.hovered() {
                        let hover_rect = response.rect;
                        ui.painter().rect_filled(
                            hover_rect,
                            6.0,
                            colors.on_surface.gamma_multiply(0.08),
                        );
                    }

                    if response.clicked() {
                        (ctx.event_callback)(id, Some(tab.clone()));
                    }

                    // Pointer cursor on hover
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
            });
        });
}

pub fn render_toolbar(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    buttons: &[arclain_plugins::types::ToolbarButton],
) {
    let colors = ctx.colors;
    let make_button = |label: &str, primary: bool| {
        arclain_widgets::TextButton::new(
            label.to_string(),
            if primary {
                arclain_widgets::ButtonSize::Medium
            } else {
                arclain_widgets::ButtonSize::Small
            },
        )
        .with_theme_colors(colors)
    };
    ui.horizontal(|ui| {
        for btn in buttons {
            // Add flexible space before this button if requested
            if btn.spacer_before {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Render remaining buttons right-to-left
                    for rbtn in buttons.iter().rev() {
                        if !rbtn.spacer_before {
                            continue; // Skip buttons before spacer
                        }

                        if ui.add(make_button(&rbtn.label, rbtn.primary)).clicked() {
                            (ctx.event_callback)(&rbtn.id, None);
                        }
                    }
                });
                return; // Done rendering
            }

            if ui.add(make_button(&btn.label, btn.primary)).clicked() {
                (ctx.event_callback)(&btn.id, None);
            }
        }
    });
}

/// Render a visually-grouped settings section (matches the host's
/// `Form/SettingsGroup` styling). The `walk_body` callback receives a
/// per-frame `Ui` plus the inner elements and is expected to render them —
/// typically by recursing through `walk_with_groups` so nested
/// `GroupBegin`/`GroupEnd` pairs work.
pub fn render_settings_group<H, F>(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, H>,
    title: &str,
    description: &Option<String>,
    body: &[PluginUiElement],
    mut walk_body: F,
) where
    H: UiEventHandler + ?Sized,
    F: FnMut(&mut egui::Ui, &[PluginUiElement], &mut RenderContext<'_, H>),
{
    let colors = ctx.colors;
    let description = description.clone();
    SettingsGroup::new(title)
        .content(|ui, group_colors| {
            if let Some(desc) = description {
                ui.label(
                    egui::RichText::new(desc)
                        .size(12.0)
                        .color(group_colors.on_surface_variant),
                );
                ui.add_space(6.0);
            }
            walk_body(ui, body, ctx);
        })
        .show(ui, colors);
}

/// Render a section header with semantic title hierarchy (h1-h4 style)
/// Uses the shared SectionHeader component for consistency
pub fn render_section_header(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    title: &str,
    level: u32,
    description: &Option<String>,
) {
    let mut header = SectionHeader::new(title).level(level);
    if let Some(desc) = description {
        header = header.description(desc);
    }
    header.show(ui, ctx.colors);
}
