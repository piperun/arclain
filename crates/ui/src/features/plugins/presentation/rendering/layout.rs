//! Layout elements for plugins

use super::context::{RenderContext, UiEventHandler};
use arclain_plugins::types::PluginUiElement;
use eframe::egui;

pub fn render_list_container<'a, H, F>(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'a, H>,
    items: &[PluginUiElement],
    max_height: Option<f32>,
    empty_message: &Option<String>,
    mut render_child: F,
) where
    H: UiEventHandler + ?Sized,
    F: FnMut(&mut egui::Ui, &PluginUiElement, &mut RenderContext<'a, H>),
{
    let height = max_height.unwrap_or(300.0);
    let colors = ctx.colors;

    egui::Frame::NONE
        .fill(colors.surface_variant)
        .corner_radius(6.0)
        .inner_margin(4.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(height)
                .show(ui, |ui| {
                    if items.is_empty() {
                        let msg = empty_message.as_deref().unwrap_or("No items");
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.label(egui::RichText::new(msg).color(colors.on_surface_variant));
                            ui.add_space(40.0);
                        });
                    } else {
                        for item in items {
                            render_child(ui, item, ctx);
                            ui.add_space(2.0);
                        }
                    }
                });
        });
}

pub fn render_separator(ui: &mut egui::Ui) {
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
}

pub fn render_space(ui: &mut egui::Ui, size: f32) {
    ui.add_space(size);
}
