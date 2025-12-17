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
                    .color(theme.colors.on_surface_variant),
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
                            theme.colors.primary
                        } else {
                            theme.colors.on_surface
                        }),
                )
                .fill(if is_selected {
                    theme.colors.surface
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
                    theme.colors.surface.linear_multiply(0.5),
                );
            }
        }
    });

    selected_page
}

/// Render the settings page header with breadcrumb and global save button
/// Returns true if the global save button was clicked
pub fn render_settings_header<F>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    current_page: &SettingsPage,
    has_changes: bool,
    custom_actions: Option<F>,
) -> bool
where
    F: FnOnce(&mut egui::Ui),
{
    // We need to capture save_clicked in a closure, but save_clicked is a bool on stack.
    // We can use a RefCell or just an Option to capture output?
    // Actually, `render_settings_header` takes `FnOnce`. We can't mutate `save_clicked` easily if we need to return it after the call.
    // Wait, the shared `render_settings_header` doesn't return anything.
    // But we can pass a closure that *sets* an external flag?
    // No, `FnOnce` consumes captured variables.
    // A simplified approach: Use an internal Cell or similar?
    // Or better: `render_settings_header` (shared) should probably just take `on_save`.
    // We can't easily return bool from the shared component if it's void.
    // However, we can use `std::cell::Cell` to capture the click.

    let save_clicked_cell = std::cell::Cell::new(false);

    let mut header = crate::shared::components::SettingsHeader::new(current_page.display_name())
        .icon(current_page.icon())
        .description(current_page.description())
        .has_changes(has_changes)
        .on_save(|| {
            save_clicked_cell.set(true);
        });

    if let Some(actions) = custom_actions {
        header = header.custom_actions(actions);
    }

    header.show(ui, theme);

    save_clicked_cell.get()
}

/// Render the settings overview page (landing page)
pub fn render_settings_overview(ui: &mut egui::Ui, theme: &AppTheme) -> Option<SettingsPage> {
    let mut selected_page = None;

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

        // Responsive settings cards - Explicit grid calculation
        let available_width = ui.available_width();
        let card_width = 280.0;
        let spacing = 16.0;
        let num_cols = ((available_width + spacing) / (card_width + spacing)).floor() as usize;
        let num_cols = num_cols.max(1);

        egui::Grid::new("settings_overview_grid")
            .spacing(egui::vec2(spacing, spacing))
            .show(ui, |ui| {
                for (index, page) in SettingsPage::all_pages().into_iter().enumerate() {
                    let card_response = egui::Frame::NONE
                        .fill(theme.colors.surface_variant)
                        .stroke(egui::Stroke::new(1.0, theme.colors.outline))
                        .corner_radius(8.0)
                        .inner_margin(20.0)
                        .show(ui, |ui| {
                            ui.set_width(card_width);
                            ui.set_height(100.0);

                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(0.0, 8.0);

                                ui.label(egui::RichText::new(page.icon()).size(24.0));

                                ui.label(
                                    egui::RichText::new(page.display_name())
                                        .size(16.0)
                                        .strong()
                                        .color(theme.colors.on_surface),
                                );

                                ui.label(
                                    egui::RichText::new(page.description())
                                        .size(12.0)
                                        .color(theme.colors.on_surface_variant),
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
                            theme.colors.primary.linear_multiply(0.1),
                        );
                    }

                    if (index + 1) % num_cols == 0 {
                        ui.end_row();
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
    crate::shared::components::Breadcrumbs::new(breadcrumb).show(ui, theme)
}

/// Render settings search results
pub fn render_settings_search_results(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    query: &str,
) -> Option<SettingsPage> {
    let mut selected_page = None;
    let query = query.to_lowercase();

    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(format!("Search results for \"{}\"", query))
                .size(14.0)
                .color(theme.colors.on_surface_variant),
        );
        ui.add_space(16.0);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(16.0, 16.0);

            let mut found = false;
            for page in SettingsPage::all_pages() {
                if page.display_name().to_lowercase().contains(&query)
                    || page.description().to_lowercase().contains(&query)
                {
                    found = true;
                    let card_response = egui::Frame::NONE
                        .fill(theme.colors.surface_variant)
                        .stroke(egui::Stroke::new(1.0, theme.colors.outline))
                        .corner_radius(8.0)
                        .inner_margin(20.0)
                        .show(ui, |ui| {
                            ui.set_width(280.0);
                            ui.set_height(80.0);

                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(page.icon()).size(20.0));
                                    ui.label(
                                        egui::RichText::new(page.display_name())
                                            .size(15.0)
                                            .strong()
                                            .color(theme.colors.on_surface),
                                    );
                                });
                                ui.label(
                                    egui::RichText::new(page.description())
                                        .size(12.0)
                                        .color(theme.colors.on_surface_variant),
                                );
                            });
                        })
                        .response;

                    if card_response.interact(egui::Sense::click()).clicked() {
                        selected_page = Some(page);
                    }

                    if card_response.hovered() {
                        ui.painter().rect_filled(
                            card_response.rect,
                            8.0,
                            theme.colors.primary.linear_multiply(0.1),
                        );
                    }
                }
            }

            if !found {
                ui.label(
                    egui::RichText::new("No matching settings found.")
                        .size(14.0)
                        .color(theme.colors.on_surface_variant)
                        .italics(),
                );
            }
        });
    });

    selected_page
}
