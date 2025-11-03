use crate::features::theme::AppTheme;
use eframe::egui;

pub struct ModalParams {
    pub width_frac: f32,
    pub height_frac: f32,
    pub min: egui::Vec2,
    pub max: egui::Vec2,
    pub padding: egui::Vec2,
    pub bottom_bar_height: f32,
    pub overlay_alpha: u8,
    pub overlay_order: egui::Order,
    pub modal_order: egui::Order,
}

impl Default for ModalParams {
    fn default() -> Self {
        Self {
            width_frac: 0.6,
            height_frac: 0.6,
            min: egui::vec2(480.0, 360.0),
            max: egui::vec2(1200.0, 900.0),
            padding: egui::vec2(20.0, 16.0),
            bottom_bar_height: 48.0,
            overlay_alpha: 180,
            overlay_order: egui::Order::Middle,
            modal_order: egui::Order::Foreground,
        }
    }
}

pub fn show_dimmed_modal(
    ctx: &egui::Context,
    theme: &AppTheme,
    id_prefix: &str,
    params: &ModalParams,
    mut content_ui: impl FnMut(&mut egui::Ui, egui::Rect),
    mut bottom_bar_ui: impl FnMut(&mut egui::Ui),
) {
    // Overlay that captures all input to block background interaction
    egui::Area::new(egui::Id::new(format!("{id_prefix}_overlay")))
        .order(params.overlay_order)
        .show(ctx, |ui| {
            let screen = ctx.screen_rect();
            ui.painter().rect_filled(
                screen,
                0.0,
                egui::Color32::from_black_alpha(params.overlay_alpha),
            );
            // Sense all input on the overlay to block interaction with content behind it
            ui.allocate_rect(screen, egui::Sense::click_and_drag());
        });

    // Modal area
    egui::Area::new(egui::Id::new(format!("{id_prefix}_modal")))
        .order(params.modal_order)
        .show(ctx, |ui| {
            let screen = ctx.screen_rect();
            let width = (screen.width() * params.width_frac).clamp(params.min.x, params.max.x);
            let height = (screen.height() * params.height_frac).clamp(params.min.y, params.max.y);
            let pos = egui::pos2((screen.width() - width) / 2.0, (screen.height() - height) / 2.0);
            let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));

            ui.painter().rect_filled(rect, 8.0, theme.colors.bg_primary);
            ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(1.0, theme.colors.border_color));
            ui.set_clip_rect(rect);

            let content = rect.shrink2(params.padding);

            let scroll_rect = egui::Rect::from_min_max(
                content.min,
                egui::pos2(content.max.x, content.max.y - params.bottom_bar_height - 6.0),
            );
            let bottom_rect = egui::Rect::from_min_max(
                egui::pos2(content.min.x, content.max.y - params.bottom_bar_height),
                content.max,
            );

            // Scrollable content area
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(scroll_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            child.vertical(|ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        content_ui(ui, content);
                    });
            });

            // Fixed bottom bar
            let mut bar = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(bottom_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            bar.horizontal(|ui| {
                // subtle top separator
                let bar_rect = ui.max_rect();
                let sep_rect = egui::Rect::from_min_max(
                    egui::pos2(bar_rect.min.x, bar_rect.min.y - 6.0),
                    egui::pos2(bar_rect.max.x, bar_rect.min.y - 5.0),
                );
                ui.painter().rect_filled(sep_rect, 0.0, theme.colors.border_color);

                bottom_bar_ui(ui);
            });
        });
}
