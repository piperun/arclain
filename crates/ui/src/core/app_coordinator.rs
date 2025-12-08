#![allow(dead_code)]
use crate::core::navigation::SettingsPage;
use crate::features::{
    archive_browser, archive_operations, organization, password_management, plugins, settings,
};
use crate::shared::SharedState;
use eframe::egui;

#[derive(Debug, Clone, PartialEq)]
pub enum AppPage {
    Main,
    Settings(SettingsPage),
    Plugins,
    Organize,
}

pub struct AppCoordinator {
    pub shared: SharedState,

    // Feature modules
    settings: settings::SettingsFeature,
    plugins: plugins::PluginsFeature,
    passwords: password_management::PasswordFeature,
    organization: organization::OrganizationFeature,
    browser: archive_browser::ArchiveBrowser,
    operations: archive_operations::ArchiveOperations,

    // Navigation
    current_page: AppPage,
    page_history: Vec<AppPage>,
}

impl AppCoordinator {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let shared = SharedState::new(cc);

        let settings = settings::SettingsFeature::new(&shared);
        let plugins = plugins::PluginsFeature::new(&shared);
        let passwords = password_management::PasswordFeature::new(&shared);
        let organization = organization::OrganizationFeature::new(&shared);
        let browser = archive_browser::ArchiveBrowser::new(&shared);
        let operations = archive_operations::ArchiveOperations::new(&shared);

        Self {
            shared,
            settings,
            plugins,
            passwords,
            organization,
            browser,
            operations,
            current_page: AppPage::Main,
            page_history: Vec::new(),
        }
    }

    pub fn navigate_to(&mut self, page: AppPage) {
        if self.current_page != page {
            self.page_history.push(self.current_page.clone());
        }
        self.current_page = page;
    }

    pub fn navigate_back(&mut self) {
        if let Some(page) = self.page_history.pop() {
            self.current_page = page;
        }
    }

    pub fn can_navigate_back(&self) -> bool {
        !self.page_history.is_empty()
    }
}

impl eframe::App for AppCoordinator {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply theme
        self.shared.theme.apply_to_context(ctx);

        // Handle password dialogs (shown on all pages)
        let pw_action =
            password_management::handle_password_dialogs(&mut self.passwords, ctx, &self.shared);
        if let password_management::PasswordFeatureAction::PasswordUnlocked { .. } = pw_action {
            // Handle password unlock success
            // This would trigger archive reload or file operation retry
        }

        // Update background operations
        self.operations.update_extraction_progress(ctx);
        self.operations.update_conversion_progress(ctx);

        // Render current page
        match &mut self.current_page {
            AppPage::Main => {
                let action = self.browser.render(ctx, &self.shared);
                match action {
                    archive_browser::ArchiveBrowserAction::NavigateToFolder(_folder) => {
                        // Handle folder navigation
                    }
                    archive_browser::ArchiveBrowserAction::OpenFile(_file) => {
                        // Handle file open
                    }
                    archive_browser::ArchiveBrowserAction::EditFile(_file) => {
                        // Handle file edit
                    }
                    archive_browser::ArchiveBrowserAction::DeleteFile(_file) => {
                        // Handle file delete
                    }
                    _ => {}
                }
            }
            AppPage::Plugins => {
                self.plugins.render(ctx, &self.shared);
            }
            AppPage::Settings(settings_page) => {
                let mut on_back = false;
                // TODO: Implement breadcrumb for AppCoordinator
                let breadcrumb = vec![];
                self.settings.render(
                    ctx,
                    &self.shared,
                    settings_page,
                    &mut on_back,
                    breadcrumb,
                    Some(&mut self.organization.rules_page),
                );
                if on_back {
                    self.navigate_back();
                }
            }
            AppPage::Organize => {
                let action = self.organization.render(ctx, &self.shared);
                match action {
                    organization::OrganizationAction::Apply => {
                        // Apply organization and navigate back
                        self.navigate_back();
                    }
                    organization::OrganizationAction::Cancel => {
                        self.navigate_back();
                    }
                    _ => {}
                }
            }
        }
    }
}
