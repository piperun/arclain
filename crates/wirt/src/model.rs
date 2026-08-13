use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginExtensionPoint {
    MainPage,
    PluginButton,
    Panel,
    Dialog(String),
    Page(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "type")]
pub enum ButtonAction {
    #[default]
    None,
    ShowDialog {
        id: String,
    },
    CloseDialog,
    OpenPage {
        id: String,
    },
    ClosePage,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginUiElement {
    Label {
        text: String,
        #[serde(default)]
        role: TextRole,
    },
    SectionHeader {
        title: String,
        level: u32,
        #[serde(default)]
        description: Option<String>,
    },
    Button {
        id: String,
        label: String,
        #[serde(default)]
        action: Option<ButtonAction>,
    },
    TextInput {
        id: String,
        label: String,
        value: String,
        #[serde(default)]
        placeholder: Option<String>,
    },
    Checkbox {
        id: String,
        label: String,
        checked: bool,
    },
    RadioGroup {
        id: String,
        label: String,
        options: Vec<String>,
        selected: String,
    },
    Slider {
        id: String,
        label: String,
        value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
    },
    Dropdown {
        id: String,
        label: String,
        options: Vec<String>,
        selected: String,
    },
    Image {
        #[serde(default)]
        cache_key: Option<String>,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        height: Option<SizeHint>,
    },
    Separator,
    Space {
        #[serde(default)]
        step: SpacingStep,
    },
    Tabs {
        id: String,
        tabs: Vec<String>,
        selected: String,
    },
    ListItem {
        id: String,
        title: String,
        #[serde(default)]
        subtitle: Option<String>,
        #[serde(default)]
        badge: Option<String>,
        #[serde(default)]
        image_key: Option<String>,
        #[serde(default)]
        image_url: Option<String>,
        #[serde(default)]
        selected: bool,
        #[serde(default)]
        warning_icon: Option<WarningIcon>,
    },
    ListContainer {
        id: String,
        items: Vec<PluginUiElement>,
        #[serde(default)]
        height: Option<SizeHint>,
        #[serde(default)]
        empty_message: Option<String>,
    },
    Loading {
        #[serde(default)]
        message: Option<String>,
    },
    GroupBegin {
        title: String,
        #[serde(default)]
        description: Option<String>,
    },
    GroupEnd,
    Warning {
        icon: WarningIcon,
        message: String,
    },
    TagChips {
        tags: Vec<String>,
        #[serde(default)]
        max_display: Option<u32>,
    },
    Toolbar {
        buttons: Vec<ToolbarButton>,
    },
    Carousel {
        id: String,
        images: Vec<(String, Option<String>)>,
        current_index: usize,
        #[serde(default)]
        height: Option<SizeHint>,
        #[serde(default = "default_true")]
        enable_lightbox: bool,
    },
    KeyValueList {
        items: Vec<KeyValuePair>,
        #[serde(default)]
        columns: Option<u32>,
    },
    MetadataGrid {
        items: Vec<KeyValuePair>,
        #[serde(default)]
        columns: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyValuePair {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolbarButton {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub spacer_before: bool,
}

/// How much room a plugin wants between two elements. The host owns the
/// pixel value for each step, so a density change moves every gap at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpacingStep {
    Small,
    #[default]
    Medium,
    Large,
}

/// What a piece of text IS, not how large it is. The host owns the type
/// scale, so restyling moves every label at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextRole {
    Title,
    Subtitle,
    #[default]
    Body,
    Caption,
    Emphasis,
}

/// How much vertical room an element wants. The host owns the pixel value
/// per element kind, so `Tall` means one thing for an image and another for
/// a list.
///
/// Deliberately not [`Default`]: the absent hint is a real case that means
/// "the host decides", and what the host decides differs per kind, so there
/// is no one variant an absent hint could stand for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeHint {
    Compact,
    Regular,
    Tall,
}

/// How much of the pane a split's sidebar wants. The host owns the pixel
/// width, so restyling moves every plugin's sidebar at once.
///
/// Deliberately not [`Default`]: the absent width is a real case that means
/// "the host decides", and it stays its own case rather than collapsing
/// onto `Medium`. The two resolve to the same number today, but one is a
/// number the host is free to move and the other is what a plugin asked
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarWidth {
    Narrow,
    Medium,
    Wide,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WarningIcon {
    Warning,
    GlobeX,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginAction {
    None,
    ShowToast {
        message: String,
        level: ToastLevel,
    },
    RefreshPanel {
        extension_point: String,
    },
    CloseDialog,
    CopyToClipboard {
        text: String,
    },
    OpenLightbox {
        images: Vec<(String, Option<String>)>,
        start_index: usize,
        title: Option<String>,
    },
    SetPageDisplayName {
        name: String,
    },
    RequestFetch {
        key: String,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BadgeConfig {
    pub count: Option<u32>,
    pub dot: bool,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopTabConfig {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub badge: Option<BadgeConfig>,
    pub priority: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginLayout {
    Single {
        elements: Vec<PluginUiElement>,
    },
    Split {
        sidebar: Vec<PluginUiElement>,
        content: Vec<PluginUiElement>,
        #[serde(default)]
        width: Option<SidebarWidth>,
    },
}

impl Default for PluginLayout {
    fn default() -> Self {
        Self::Single {
            elements: Vec::new(),
        }
    }
}

impl PluginLayout {
    pub fn elements(&self) -> Vec<&PluginUiElement> {
        match self {
            Self::Single { elements } => elements.iter().collect(),
            Self::Split {
                sidebar, content, ..
            } => sidebar.iter().chain(content).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Single { elements } => elements.is_empty(),
            Self::Split {
                sidebar, content, ..
            } => sidebar.is_empty() && content.is_empty(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Single { elements } => elements.len(),
            Self::Split {
                sidebar, content, ..
            } => sidebar.len() + content.len(),
        }
    }

    pub fn flatten(self) -> Vec<PluginUiElement> {
        match self {
            Self::Single { elements } => elements,
            Self::Split {
                mut sidebar,
                mut content,
                ..
            } => {
                sidebar.append(&mut content);
                sidebar
            }
        }
    }
}
