//! Main content handler for ArclainApp

use super::ArclainApp;
use crate::core::navigation::{AppPage, PageNavigator};
use eframe::egui;

pub fn render_content(app: &mut ArclainApp, ctx: &egui::Context) {
    // Check for plugin page first - if open, render it instead of normal content
    if crate::features::plugins::render_page(ctx, &app.shared_state) {
        // Plugin page handled content, skip normal rendering
        return;
    }

    // Render Main Content
    egui::CentralPanel::default().show(ctx, |_ui| {
        let current_page = app.page_navigator.current_page.clone();
        match current_page {
            AppPage::Main => {
                let shared_state = app.shared_state.clone();

                let action = app.archive_browser.render(ctx, &shared_state);

                // Handle actions via BrowserController
                app.archive_browser.controller.handle_action(
                    action,
                    &shared_state,
                    app.archive_operations.state_mut(),
                    &mut app.organization_feature,
                    &mut app.page_navigator,
                    ctx,
                );
            }
            AppPage::Settings(page) => {
                let breadcrumb = PageNavigator::get_breadcrumb(&AppPage::Settings(page.clone()));
                // Get search_text from signal
                let search_text = app.shared_state.signals().search_text.get();
                egui::CentralPanel::default().show(ctx, |ui| {
                    if let Some(target) = app.settings_feature.render(
                        ui,
                        &app.shared_state,
                        &page,
                        breadcrumb,
                        Some(&mut app.organization_feature.rules_page),
                        &search_text,
                    ) {
                        app.page_navigator.navigate_to(target);
                    }
                });
            }
            AppPage::Plugins => {
                app.plugins_feature.render(ctx, &app.shared_state);
            }
            AppPage::Organize => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // Get OrganizationService from Services container
                    if let Some(org_service) =
                        app.shared_state.services.organization_service.as_ref()
                    {
                        app.organization_feature.rules_page.render(
                            ui,
                            &app.shared_state.theme,
                            org_service,
                        );
                    } else {
                        ui.label("Organization service not available.");
                    }
                });
            }
            AppPage::OrganizeArchive(_name) => {
                let shared_state = app.shared_state.clone();
                let action = app.organization_feature.render(ctx, &shared_state);

                let mut status_info = shared_state.signals().status_bar.get();
                let mut action_ctx = crate::features::organization::presentation::controllers::organization_controller::ActionContext {

                    shared: &shared_state,
                    organization_feature: &mut app.organization_feature,
                    page_navigator: &mut app.page_navigator,
                    status_info: &mut status_info,
                };
                action_ctx.handle(&action);
                shared_state.signals().status_bar.set(status_info);
            }
        }
    });
}
