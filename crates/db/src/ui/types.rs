//! UI configuration data types
//!
//! Pure data structures shared by the rusqlite (`config.rs`) and
//! Diesel (`diesel_ops.rs`) sides of the UI config feature, plus
//! the seed defaults (`seed.rs`).

/// Display mode for UI elements
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayMode {
    #[default]
    IconAndText,
    IconOnly,
    TextOnly,
}

impl DisplayMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            DisplayMode::IconAndText => "icon_and_text",
            DisplayMode::IconOnly => "icon_only",
            DisplayMode::TextOnly => "text_only",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "icon_only" => DisplayMode::IconOnly,
            "text_only" => DisplayMode::TextOnly,
            _ => DisplayMode::IconAndText,
        }
    }
}

/// Action type for UI items
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionType {
    #[default]
    Builtin,
    Plugin,
    Custom,
}

impl ActionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionType::Builtin => "builtin",
            ActionType::Plugin => "plugin",
            ActionType::Custom => "custom",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "plugin" => ActionType::Plugin,
            "custom" => ActionType::Custom,
            _ => ActionType::Builtin,
        }
    }
}

/// UI region identifier
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiRegion {
    Toolbar,
    ContextMenu,
    ToolsDialog,
    InfoPanel,
}

impl UiRegion {
    pub fn as_str(&self) -> &'static str {
        match self {
            UiRegion::Toolbar => "toolbar",
            UiRegion::ContextMenu => "context_menu",
            UiRegion::ToolsDialog => "tools_dialog",
            UiRegion::InfoPanel => "info_panel",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "toolbar" => Some(UiRegion::Toolbar),
            "context_menu" => Some(UiRegion::ContextMenu),
            "tools_dialog" => Some(UiRegion::ToolsDialog),
            "info_panel" => Some(UiRegion::InfoPanel),
            _ => None,
        }
    }
}

/// A UI item (button, menu item, etc.)
#[derive(Clone, Debug)]
pub struct UiItem {
    pub id: String,
    pub region: UiRegion,
    pub group_id: Option<String>,
    pub label: String,
    pub icon: Option<String>,
    pub visible: bool,
    pub sort_order: i32,
    pub display_mode: DisplayMode,
    pub action_type: ActionType,
    pub action_data: Option<String>,
}

/// Region-level configuration
#[derive(Clone, Debug)]
pub struct UiRegionConfig {
    pub id: String,
    pub enabled: bool,
    pub global_display_mode: Option<DisplayMode>,
}
