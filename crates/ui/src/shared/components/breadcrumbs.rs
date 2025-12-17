use crate::shared::theme::AppTheme;
use eframe::egui;

pub struct Breadcrumbs<'a> {
    items: &'a [(String, crate::core::AppPage)],
}

impl<'a> Breadcrumbs<'a> {
    pub fn new(items: &'a [(String, crate::core::AppPage)]) -> Self {
        Self { items }
    }

    pub fn show(self, ui: &mut egui::Ui, theme: &AppTheme) -> Option<crate::core::AppPage> {
        let mut navigate_to = None;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);

            // Home button with Phosphor icon
            let home_btn = egui::Button::new(
                egui::RichText::new(format!("{} Home", egui_phosphor::regular::HOUSE))
                    .size(13.0)
                    .color(theme.colors.on_surface_variant),
            )
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
            .frame(false);

            if ui.add(home_btn).clicked() {
                navigate_to = Some(crate::core::AppPage::Main);
            }

            for (i, (label, page)) in self.items.iter().enumerate() {
                ui.label(
                    egui::RichText::new(">")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );

                let is_last = i == self.items.len() - 1;

                if is_last {
                    // Current page - not clickable
                    ui.label(
                        egui::RichText::new(label.as_str())
                            .size(13.0)
                            .color(theme.colors.on_surface),
                    );
                } else {
                    // Previous pages - clickable
                    let btn = egui::Button::new(
                        egui::RichText::new(label.as_str())
                            .size(13.0)
                            .color(theme.colors.on_surface_variant),
                    )
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::NONE)
                    .frame(false);

                    if ui.add(btn).clicked() {
                        navigate_to = Some(page.clone());
                    }
                }
            }
        });

        navigate_to
    }
}
