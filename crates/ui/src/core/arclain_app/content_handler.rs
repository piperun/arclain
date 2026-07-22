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
                    let borrows = crate::features::settings::SettingsFeatureBorrows {
                        rules_page: Some(&mut app.organization_feature.rules_page),
                        profiles_page: Some(&mut app.organization_feature.profiles_page),
                        hotkeys: Some(&mut app.hotkeys_feature),
                        password_management: Some(&mut app.password_management_feature),
                        plugins: Some(&mut app.plugins_feature),
                    };
                    if let Some(target) = app.settings_feature.render(
                        ui,
                        &app.shared_state,
                        &page,
                        breadcrumb,
                        borrows,
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
                        if let Some(action) = app
                            .organization_feature
                            .rules_page
                            .render(ui, &app.shared_state.theme)
                        {
                            use crate::features::organization::presentation::views::RulesPageAction;
                            match action {
                                RulesPageAction::Navigate(_page) => {
                                    // Top-level Organize view doesn't drive
                                    // SettingsPage navigation directly — the
                                    // Edit-rule flow lives under Settings, not
                                    // this top-level page. Ignore Navigate
                                    // intents here.
                                }
                                other => {
                                    let user_config =
                                        app.shared_state.signals().user_config.get();
                                    let plugins = app
                                        .shared_state
                                        .plugin_ui_jobs
                                        .plugin_snapshot(&user_config);
                                    crate::features::organization::presentation::views::rules_page::handle_rules_page_action(
                                        &mut app.organization_feature.rules_page,
                                        other,
                                        org_service,
                                        plugins.as_deref().map(Vec::as_slice),
                                    );
                                }
                            }
                        }
                    } else {
                        ui.label("Organization service not available.");
                    }
                });
            }
            AppPage::Logs => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let network_logs = app
                        .shared_state
                        .plugin_ui_jobs
                        .network_log()
                        .unwrap_or_default();

                    crate::shared::components::logs_page::LogsPage::render_page(
                        ui,
                        network_logs.as_ref(),
                        &mut app.logs_page_state,
                        &app.shared_state.theme.colors,
                    );
                });
            }
            AppPage::Process => {
                if let Some(action) = crate::features::process::view::render(
                    ctx,
                    &app.shared_state,
                    &mut app.process_state,
                ) {
                    crate::features::process::view::handle_process_action(
                        &mut app.process_state,
                        action,
                        &app.shared_state,
                    );
                }
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
                // status_bar write happens after action handler completes,
                // not during render — controller-pattern write, not the
                // render-mutate-write smell the audit flagged. Kept as
                // correct (audit B3 reframing).
                shared_state.signals().status_bar.set_if_changed(status_info);
            }
        }
    });
}
