use eframe::egui;

/// Configuration for the Settings Header
/// Each page (or sub-page state) produces this config to tell the parent container how to render the header.
pub struct SettingsHeaderConfig<'a> {
    pub title: String,
    pub icon: Option<String>,
    pub description: Option<String>,
    pub sub_description: Option<String>,
    /// If true, the "Save" button (if applicable to the page type) will be highlighted or enabled.
    pub has_changes: bool,

    /// Optional "Back" action. If present, a back button is shown.
    pub on_back: Option<Box<dyn FnOnce() + 'a>>,

    /// Optional save action. If present, enables the save button.
    pub on_save: Option<Box<dyn FnOnce() + 'a>>,

    /// Optional custom actions to be rendered in the main header row (right side).
    pub custom_actions: Option<Box<dyn FnOnce(&mut egui::Ui) + 'a>>,

    /// Optional secondary row content (below title).
    pub secondary_row: Option<Box<dyn FnOnce(&mut egui::Ui) + 'a>>,

    /// Optional tertiary row content (below secondary).
    pub tertiary_row: Option<Box<dyn FnOnce(&mut egui::Ui) + 'a>>,
}

impl<'a> Default for SettingsHeaderConfig<'a> {
    fn default() -> Self {
        Self {
            title: "Settings".to_string(),
            icon: None,
            description: None,
            sub_description: None,
            has_changes: false,
            on_back: None,
            on_save: None,
            custom_actions: None,
            secondary_row: None,
            tertiary_row: None,
        }
    }
}

impl<'a> SettingsHeaderConfig<'a> {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn sub_description(mut self, sub_desc: impl Into<String>) -> Self {
        self.sub_description = Some(sub_desc.into());
        self
    }

    pub fn has_changes(mut self, has_changes: bool) -> Self {
        self.has_changes = has_changes;
        self
    }

    pub fn on_back(mut self, action: impl FnOnce() + 'a) -> Self {
        self.on_back = Some(Box::new(action));
        self
    }

    pub fn on_save(mut self, action: impl FnOnce() + 'a) -> Self {
        self.on_save = Some(Box::new(action));
        self
    }

    #[allow(dead_code)] // Part of public API for future page customization
    pub fn custom_actions(mut self, actions: impl FnOnce(&mut egui::Ui) + 'a) -> Self {
        self.custom_actions = Some(Box::new(actions));
        self
    }

    pub fn secondary_row(mut self, row: impl FnOnce(&mut egui::Ui) + 'a) -> Self {
        self.secondary_row = Some(Box::new(row));
        self
    }

    pub fn tertiary_row(mut self, row: impl FnOnce(&mut egui::Ui) + 'a) -> Self {
        self.tertiary_row = Some(Box::new(row));
        self
    }
}
