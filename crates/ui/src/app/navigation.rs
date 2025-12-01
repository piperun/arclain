use serde::{Deserialize, Serialize};

/// Represents the different pages in the application
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppPage {
    /// Main archive viewer page
    Main,
    /// Plugins management page
    Plugins,
    /// Settings page with a specific category selected
    /// Settings page with a specific category selected
    Settings(SettingsPage),
    /// Archive organization page
    Organize,
}

/// Represents different settings categories
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingsPage {
    /// Overview/landing page for settings
    Overview,
    /// General application settings
    General,
    /// Archive-related settings
    Archives,
    /// Password rules management
    PasswordRules,
    /// Organization rules management
    OrganizationRules,
    /// Security and encryption settings
    Security,
    /// Plugin management
    Plugins,
}

impl SettingsPage {
    /// Get display name for the settings page
    pub fn display_name(&self) -> &'static str {
        match self {
            SettingsPage::Overview => "Settings",
            SettingsPage::General => "General",
            SettingsPage::Archives => "Archives",
            SettingsPage::PasswordRules => "Password Rules",
            SettingsPage::OrganizationRules => "Organization Rules",
            SettingsPage::Security => "Security",
            SettingsPage::Plugins => "Plugins",
        }
    }

    /// Get icon for the settings page
    pub fn icon(&self) -> &'static str {
        match self {
            SettingsPage::Overview => "⚙",
            SettingsPage::General => "🔧",
            SettingsPage::Archives => "📦",
            SettingsPage::PasswordRules => "🔐",
            SettingsPage::OrganizationRules => "📋",
            SettingsPage::Security => "🛡",
            SettingsPage::Plugins => "⬢",
        }
    }

    /// Get description for the settings page
    pub fn description(&self) -> &'static str {
        match self {
            SettingsPage::Overview => "Configure application settings",
            SettingsPage::General => "General application preferences",
            SettingsPage::Archives => "Archive handling and extraction options",
            SettingsPage::PasswordRules => "Manage password rules and patterns",
            SettingsPage::OrganizationRules => "Manage archive organization rules",
            SettingsPage::Security => "Encryption and security settings",
            SettingsPage::Plugins => "Manage and configure plugins",
        }
    }

    /// Get all available settings pages (excluding Overview)
    pub fn all_pages() -> Vec<SettingsPage> {
        vec![
            SettingsPage::General,
            SettingsPage::Archives,
            SettingsPage::PasswordRules,
            SettingsPage::OrganizationRules,
            SettingsPage::Security,
            SettingsPage::Plugins,
        ]
    }
}

impl Default for AppPage {
    fn default() -> Self {
        AppPage::Main
    }
}

/// Navigation state manager for the application
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageNavigator {
    /// Current page being displayed
    pub current_page: AppPage,
    /// Navigation history stack (for back button)
    history: Vec<AppPage>,
    /// Maximum history size
    max_history: usize,
}

impl Default for PageNavigator {
    fn default() -> Self {
        Self {
            current_page: AppPage::Main,
            history: Vec::new(),
            max_history: 20,
        }
    }
}

impl PageNavigator {
    /// Create a new page navigator
    pub fn new() -> Self {
        Self::default()
    }

    /// Navigate to a new page, adding current page to history
    pub fn navigate_to(&mut self, page: AppPage) {
        // Don't add to history if we're going to the same page
        if self.current_page != page {
            // Add current page to history
            self.history.push(self.current_page.clone());

            // Limit history size
            if self.history.len() > self.max_history {
                self.history.remove(0);
            }
        }

        self.current_page = page;
    }

    /// Navigate back to previous page
    pub fn navigate_back(&mut self) -> bool {
        if let Some(previous) = self.history.pop() {
            self.current_page = previous;
            true
        } else {
            false
        }
    }

    /// Check if we can navigate back
    pub fn can_go_back(&self) -> bool {
        !self.history.is_empty()
    }

    /// Navigate to main page, clearing history
    pub fn navigate_to_main(&mut self) {
        self.history.clear();
        self.current_page = AppPage::Main;
    }

    /// Get breadcrumb path for current page
    #[allow(dead_code)]
    pub fn get_breadcrumb(&self) -> Vec<(&'static str, AppPage)> {
        match &self.current_page {
            AppPage::Main => vec![],
            AppPage::Plugins => vec![("Plugins", AppPage::Plugins)],
            AppPage::Organize => vec![("Organize", AppPage::Organize)],
            AppPage::Settings(category) => {
                let mut breadcrumb = vec![("Settings", AppPage::Settings(SettingsPage::Overview))];

                if *category != SettingsPage::Overview {
                    breadcrumb.push((category.display_name(), self.current_page.clone()));
                }

                breadcrumb
            }
        }
    }

    /// Check if currently on main page
    pub fn is_on_main(&self) -> bool {
        matches!(self.current_page, AppPage::Main)
    }

    /// Check if currently on settings
    #[allow(dead_code)]
    pub fn is_on_settings(&self) -> bool {
        matches!(self.current_page, AppPage::Settings(_))
    }

    /// Get current settings page if on settings
    #[allow(dead_code)]
    pub fn current_settings_page(&self) -> Option<&SettingsPage> {
        match &self.current_page {
            AppPage::Settings(page) => Some(page),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_navigation_basic() {
        let mut nav = PageNavigator::new();
        assert!(nav.is_on_main());

        nav.navigate_to(AppPage::Settings(SettingsPage::General));
        assert!(nav.is_on_settings());
        assert!(nav.can_go_back());

        nav.navigate_back();
        assert!(nav.is_on_main());
    }

    #[test]
    fn test_navigation_history() {
        let mut nav = PageNavigator::new();

        nav.navigate_to(AppPage::Settings(SettingsPage::General));
        nav.navigate_to(AppPage::Settings(SettingsPage::Security));

        assert_eq!(nav.history.len(), 2);
        assert!(nav.can_go_back());
    }

    #[test]
    fn test_breadcrumb() {
        let mut nav = PageNavigator::new();

        // Main page has no breadcrumb
        assert_eq!(nav.get_breadcrumb().len(), 0);

        // Settings overview
        nav.navigate_to(AppPage::Settings(SettingsPage::Overview));
        assert_eq!(nav.get_breadcrumb().len(), 1);

        // Settings category
        nav.navigate_to(AppPage::Settings(SettingsPage::General));
        assert_eq!(nav.get_breadcrumb().len(), 2);
    }

    #[test]
    fn settings_breadcrumb_has_overview_and_leaf() {
        let mut nav = PageNavigator::new();
        nav.navigate_to(AppPage::Settings(SettingsPage::Archives));
        let bc = nav.get_breadcrumb();
        assert_eq!(bc.len(), 2);
        assert_eq!(bc[0].0, "Settings");
        assert_eq!(bc[1].0, SettingsPage::Archives.display_name());
    }

    #[test]
    fn settings_pages_list_has_all() {
        let pages = SettingsPage::all_pages();
        assert_eq!(pages.len(), 4);
        assert!(pages.contains(&SettingsPage::Security));
    }
}
