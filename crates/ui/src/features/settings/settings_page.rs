use crate::core::navigation::SettingsPage;
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render the settings navigator panel (left sidebar)
pub fn render_settings_navigator(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    current_page: &SettingsPage,
) -> Option<SettingsPage> {
    let mut selected_page = None;

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);

        // Title
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new("SETTINGS")
                    .size(11.0)
                    .strong()
                    .color(theme.colors.text_secondary),
            );
        });
        ui.add_space(16.0);

        // Navigation items
        for page in SettingsPage::all_pages() {
            let is_selected = current_page == &page;

            let item_response = ui.add(
                egui::Button::new(
                    egui::RichText::new(format!("{}  {}", page.icon(), page.display_name()))
                        .size(14.0)
                        .color(if is_selected {
                            theme.colors.accent
                        } else {
                            theme.colors.text_primary
                        }),
                )
                .fill(if is_selected {
                    theme.colors.bg_primary
                } else {
                    egui::Color32::TRANSPARENT
                })
                .stroke(egui::Stroke::NONE)
                .frame(false)
                .min_size(egui::vec2(240.0, 36.0)),
            );

            if item_response.clicked() {
                selected_page = Some(page);
            }

            // Hover effect
            if item_response.hovered() && !is_selected {
                let mut hover_rect = item_response.rect;
                hover_rect.set_left(hover_rect.left() + 12.0);
                hover_rect.set_right(hover_rect.right() - 8.0);
                ui.painter().rect_filled(
                    hover_rect,
                    6.0,
                    theme.colors.bg_primary.linear_multiply(0.5),
                );
            }
        }
    });

    selected_page
}

/// Render the settings page header with breadcrumb
pub fn render_settings_header(ui: &mut egui::Ui, theme: &AppTheme, current_page: &SettingsPage) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);

        // Page icon and title
        ui.label(egui::RichText::new(current_page.icon()).size(24.0));

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);

            ui.label(
                egui::RichText::new(current_page.display_name())
                    .size(20.0)
                    .strong()
                    .color(theme.colors.text_primary),
            );

            ui.label(
                egui::RichText::new(current_page.description())
                    .size(12.0)
                    .color(theme.colors.text_secondary),
            );
        });
    });

    ui.add_space(12.0);
    ui.separator();
}

/// Render the settings overview page (landing page)
pub fn render_settings_overview(ui: &mut egui::Ui, theme: &AppTheme) -> Option<SettingsPage> {
    let mut selected_page = None;

    egui::ScrollArea::vertical()
        .id_salt("settings_overview_scroll")
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

            // Settings categories grid (no duplicate header - already shown in page header)
            egui::Grid::new("settings_grid")
                .spacing([16.0, 16.0])
                .show(ui, |ui| {
                    let mut col = 0;

                    for page in SettingsPage::all_pages() {
                        let card_response = egui::Frame::NONE
                            .fill(theme.colors.bg_secondary)
                            .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
                            .corner_radius(8.0)
                            .inner_margin(20.0)
                            .show(ui, |ui| {
                                ui.set_min_size(egui::vec2(280.0, 100.0));

                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 8.0);

                                    ui.label(egui::RichText::new(page.icon()).size(24.0));

                                    ui.label(
                                        egui::RichText::new(page.display_name())
                                            .size(16.0)
                                            .strong()
                                            .color(theme.colors.text_primary),
                                    );

                                    ui.label(
                                        egui::RichText::new(page.description())
                                            .size(12.0)
                                            .color(theme.colors.text_secondary),
                                    );
                                });
                            })
                            .response;

                        if card_response.interact(egui::Sense::click()).clicked() {
                            selected_page = Some(page);
                        }

                        // Hover effect
                        if card_response.hovered() {
                            ui.painter().rect_filled(
                                card_response.rect,
                                8.0,
                                theme.colors.accent.linear_multiply(0.1),
                            );
                        }

                        col += 1;
                        if col >= 2 {
                            ui.end_row();
                            col = 0;
                        }
                    }
                });
        });

    selected_page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_settings_pages_present() {
        let pages = SettingsPage::all_pages();
        assert_eq!(pages.len(), 7);
        assert!(pages.contains(&SettingsPage::General));
        assert!(pages.contains(&SettingsPage::Archives));
        assert!(pages.contains(&SettingsPage::PasswordRules));
        assert!(pages.contains(&SettingsPage::OrganizationRules));
        assert!(pages.contains(&SettingsPage::Security));
        assert!(pages.contains(&SettingsPage::Plugins));
    }
}

/// Render breadcrumb navigation for settings
pub fn render_breadcrumb(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    breadcrumb: &[(String, crate::core::AppPage)],
) -> Option<crate::core::AppPage> {
    let mut navigate_to = None;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);

        // Home button with Phosphor icon
        let home_btn = egui::Button::new(
            egui::RichText::new(format!("{} Home", egui_phosphor::regular::HOUSE))
                .size(13.0)
                .color(theme.colors.text_secondary),
        )
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE)
        .frame(false);

        if ui.add(home_btn).clicked() {
            navigate_to = Some(crate::core::AppPage::Main);
        }

        for (i, (label, page)) in breadcrumb.iter().enumerate() {
            ui.label(
                egui::RichText::new(">")
                    .size(12.0)
                    .color(theme.colors.text_secondary),
            );

            let is_last = i == breadcrumb.len() - 1;

            if is_last {
                // Current page - not clickable
                ui.label(
                    egui::RichText::new(label.as_str())
                        .size(13.0)
                        .color(theme.colors.text_primary),
                );
            } else {
                // Previous pages - clickable
                let btn = egui::Button::new(
                    egui::RichText::new(label.as_str())
                        .size(13.0)
                        .color(theme.colors.text_secondary),
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
