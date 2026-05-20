//! Main content handler for ArclainApp

use super::ArclainApp;
use crate::core::navigation::{AppPage, PageNavigator};
use eframe::egui;

pub fn render_content(app: &mut ArclainApp, ctx: &egui::Context) {
    // Plugin pages only take over rendering when we're on the Main page.
    // If the user navigated to Logs/Settings/etc., normal content takes priority.
    if app.page_navigator.current_page == AppPage::Main {
        if crate::features::plugins::presentation::views::rendering::render_page(
            ctx,
            &app.shared_state,
        ) {
            return;
        }
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
                        Some(&mut app.organization_feature.profiles_page),
                        Some(&mut app.hotkeys_feature),
                        Some(&mut app.password_management_feature),
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
            AppPage::Logs => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let logs = if let Some(manager) = &app.shared_state.services.plugin_manager {
                        manager.lock().get_network_log()
                    } else {
                        Vec::new()
                    };

                    crate::shared::components::network_log::NetworkLog::render_page(
                        ui,
                        &logs,
                        &mut app.network_log_state,
                        &app.shared_state.theme.colors,
                    );
                });
            }
            AppPage::Process => {
                crate::features::process::view::render(
                    ctx,
                    &app.shared_state,
                    &mut app.process_state,
                );
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
                shared_state.signals().status_bar.set_if_changed(status_info);
            }
        }
    });
}
