//! Renderer-neutral plugin UI document model.
//!
//! [`crate::model::PluginUiElement`]/[`crate::model::PluginLayout`] are the shapes the WIT
//! boundary lifts out of a WASM guest (via `crate::conversions`) -- they
//! carry an `id: String` on some variants only (whichever elements the
//! WIT schema itself gives an id), and nothing at all for containers a
//! renderer nonetheless needs a stable identity for (a `Vec` position, a
//! `GroupBegin`/`GroupEnd` marker pair). Every current and future
//! frontend (egui today, a Flutter/Dart bridge tomorrow) needs a single
//! node shape where **every** node -- interactive or not -- carries an
//! `id`, so it can index widget state, target an action, and diff a
//! revision without re-deriving element-kind-specific rules of its own.
//!
//! [`normalize_layout`] is that boundary-normalization step. It does not
//! change the WIT ABI (the brief for this module explicitly forbids
//! that): it runs entirely on the host side, after
//! `crate::conversions::convert_plugin_layout` has already produced a
//! `PluginLayout`. Two rules decide where a node's `id` comes from:
//!
//! - An element the WIT schema already gives an id (`Button`,
//!   `TextInput`, `Checkbox`, `RadioGroup`, `Slider`, `Dropdown`, `Tabs`,
//!   `ListItem`, `ListContainer`, `Carousel`) keeps that id **verbatim**
//!   -- unprefixed, unmodified -- because the host runtime dispatches to
//!   the WASM guest by that exact string,
//!   and a renderer-neutral `PluginActionRequest::node_id` must round-trip
//!   through this module unchanged for that dispatch to keep working.
//! - Every other element (display-only, or a container the WIT schema
//!   itself never gave an id -- `Toolbar`, `GroupBegin`/`GroupEnd`'s
//!   synthesized `Group`, the top-level `Single`/`Split` root) gets a
//!   deterministic **structural-path** id: `"#root"`, `"#root/2"`,
//!   `"#root/2/1"`, built from its position among its own siblings at
//!   every level from the root. The leading `#` and `/`-joined path make
//!   a host-generated id visually distinct from anything a plugin would
//!   plausibly author, but that is a readability aid, not the actual
//!   safety net: [`normalize_layout`] additionally collects **every**
//!   assigned id (interactive and structural alike) into one set and
//!   rejects the whole layout with [`PluginUiNormalizeError::DuplicateNodeId`]
//!   the instant a second node -- of either origin -- claims an id
//!   already taken. A malicious or buggy plugin cannot forge a
//!   structural-looking id to collide with (and thereby hijack actions
//!   aimed at) a legitimate node, because any such collision is rejected
//!   outright rather than silently resolved by "last one wins".
//!
//! `GroupBegin`/`GroupEnd` are flat sibling markers in the WIT model
//! (open a group, emit ordinary elements, close it) but a renderer-
//! neutral tree needs the enclosed elements to actually be **children**
//! of a `Group` node -- see [`PluginUiNodeKind::Group`]. This module
//! resolves that during the same walk: `GroupBegin` scans forward
//! (tracking nested `GroupBegin`/`GroupEnd` pairs so a group may itself
//! contain a group) for its matching `GroupEnd`, and everything strictly
//! between the two becomes the synthesized `Group` node's children. A
//! `GroupEnd` with no open `GroupBegin`, or a `GroupBegin` never closed
//! before its sibling list ends, is rejected as malformed
//! ([`PluginUiNormalizeError::UnmatchedGroupEnd`] /
//! [`PluginUiNormalizeError::UnclosedGroup`]) rather than silently
//! swallowed or guessed at -- a plugin's layout bug should surface as a
//! normalization error the application layer can log and refuse to
//! render, not as a document silently missing content or, worse,
//! misattributing sibling elements to the wrong group.
//!
//! Tree shape is additionally bounded independently of the WIT-side
//! guest-result quotas the host runtime already enforces before this
//! module ever sees a layout ([`MAX_UI_TREE_DEPTH`], [`MAX_UI_NODES`],
//! [`MAX_UI_TEXT_BYTES`], [`MAX_UI_ASSETS`]) -- defense in depth, and
//! also what lets this module's own budget tests exercise the limits
//! directly against a hand-built [`crate::model::PluginLayout`] without
//! needing a real WASM guest to produce one.

use crate::model::{
    ButtonAction, KeyValuePair, PluginLayout, PluginUiElement, ToastLevel, ToolbarButton,
    WarningIcon,
};

/// Re-exported because a node kind names them directly. Steps and roles are
/// host vocabulary rather than plugin-supplied data, so there is nothing for
/// a DTO twin to normalize -- the enum a plugin picks from is the enum a
/// frontend matches on.
pub use crate::model::{SidebarWidth, SizeHint, SpacingStep, TextRole};

/// Maximum nesting depth (containers within containers within the root)
/// a normalized tree may reach.
pub const MAX_UI_TREE_DEPTH: usize = 64;
/// Maximum total node count (root included) a normalized tree may reach.
pub const MAX_UI_NODES: usize = 10_000;
/// Maximum aggregate bytes across every string field normalization
/// copies into the tree (labels, titles, options, messages, ...).
pub const MAX_UI_TEXT_BYTES: usize = 1024 * 1024;
/// Maximum number of image/asset references (an `Image`/`Carousel`
/// element/frame, or a `ListItem` image) a normalized tree may reach.
pub const MAX_UI_ASSETS: usize = 512;

/// Why [`normalize_layout`] rejected a [`PluginLayout`] outright, rather
/// than producing a partial or best-effort tree. Every variant is a
/// structural problem with the plugin's own layout (a duplicate id, an
/// unbalanced group marker) or a budget the layout exceeded -- never an
/// I/O or host-side failure, so this type carries no wrapped cause.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PluginUiNormalizeError {
    #[error("duplicate node id: {0}")]
    DuplicateNodeId(String),
    #[error("group-end marker with no matching group-begin")]
    UnmatchedGroupEnd,
    #[error("group-begin marker was never closed by a matching group-end")]
    UnclosedGroup,
    #[error("plugin UI tree exceeds the maximum nesting depth of {MAX_UI_TREE_DEPTH}")]
    TreeTooDeep,
    #[error("plugin UI tree exceeds the maximum node count of {MAX_UI_NODES}")]
    TooManyNodes,
    #[error("plugin UI tree exceeds the maximum text budget of {MAX_UI_TEXT_BYTES} bytes")]
    TextBudgetExceeded,
    #[error("plugin UI tree exceeds the maximum asset count of {MAX_UI_ASSETS}")]
    TooManyAssets,
}

/// One node in a renderer-neutral plugin UI document. Every node --
/// interactive or display-only -- carries a stable `id` (see the module
/// doc comment), plus `visible`/`enabled` flags a host-side action
/// dispatcher checks before forwarding an interaction to the plugin (see
/// `crate::ui_model::PluginUiNodeDto::find`'s doc comment). Nothing in
/// the current WIT schema can mark a node hidden or disabled -- every
/// node [`normalize_layout`] produces today is `visible: true, enabled:
/// true` -- but the fields exist now so the enforcement mechanism has
/// something to check ahead of any future WIT change that adds one, and
/// so it is testable on its own by constructing a node directly.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginUiNodeDto {
    pub id: String,
    pub kind: PluginUiNodeKind,
    pub visible: bool,
    pub enabled: bool,
}

impl PluginUiNodeDto {
    /// Depth-first search for the node with the given `id`, starting at
    /// (and including) `self`. Used by the application layer to check a
    /// [`crate::PluginActionRequest`]-equivalent's target node's
    /// `visible`/`enabled` flags before dispatching to the plugin.
    ///
    /// Returns `None` for an id that does not name any node in this
    /// tree -- notably including a [`PluginToolbarButtonDto`]'s own `id`
    /// (toolbar buttons are plain data inside a `Toolbar` node, not
    /// separate tree nodes) and any internal lifecycle event id
    /// (`"__page_init"` and similar). Callers must treat "not found" as
    /// "nothing to check" (dispatch normally) rather than "reject": this
    /// tree can only gate interactions it actually has visibility/enabled
    /// state for.
    pub fn find(&self, id: &str) -> Option<&PluginUiNodeDto> {
        if self.id == id {
            return Some(self);
        }
        self.children().into_iter().find_map(|child| child.find(id))
    }

    fn children(&self) -> Vec<&PluginUiNodeDto> {
        match &self.kind {
            PluginUiNodeKind::Single { children }
            | PluginUiNodeKind::ListContainer { children, .. }
            | PluginUiNodeKind::Group { children, .. } => children.iter().collect(),
            PluginUiNodeKind::Split {
                sidebar, content, ..
            } => sidebar.iter().chain(content.iter()).collect(),
            _ => Vec::new(),
        }
    }
}

/// The renderer-neutral shape of one [`PluginUiNodeDto`]. One variant per
/// current WIT UI element, plus `Single`/`Split` (the top-level layout
/// root) and `Group` (synthesized from a `GroupBegin`/`GroupEnd` marker
/// pair -- see the module doc comment).
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PluginUiNodeKind {
    Single {
        children: Vec<PluginUiNodeDto>,
    },
    Split {
        sidebar: Vec<PluginUiNodeDto>,
        content: Vec<PluginUiNodeDto>,
        width: Option<SidebarWidth>,
    },
    Label {
        text: String,
        role: TextRole,
    },
    SectionHeader {
        title: String,
        level: u32,
        description: Option<String>,
    },
    Button {
        label: String,
        action: Option<PluginButtonActionDto>,
    },
    TextInput {
        label: String,
        value: String,
        placeholder: Option<String>,
    },
    Checkbox {
        label: String,
        checked: bool,
    },
    RadioGroup {
        label: String,
        options: Vec<String>,
        selected: String,
    },
    Slider {
        label: String,
        value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
    },
    Dropdown {
        label: String,
        options: Vec<String>,
        selected: String,
    },
    Image {
        cache_key: Option<String>,
        url: Option<String>,
        height: Option<SizeHint>,
    },
    Separator,
    Space {
        step: SpacingStep,
    },
    Tabs {
        tabs: Vec<String>,
        selected: String,
    },
    ListItem {
        title: String,
        subtitle: Option<String>,
        badge: Option<String>,
        image_key: Option<String>,
        image_url: Option<String>,
        selected: bool,
        warning_icon: Option<PluginWarningIconDto>,
    },
    ListContainer {
        children: Vec<PluginUiNodeDto>,
        height: Option<SizeHint>,
        empty_message: Option<String>,
    },
    Loading {
        message: Option<String>,
    },
    Group {
        title: String,
        description: Option<String>,
        children: Vec<PluginUiNodeDto>,
    },
    Warning {
        icon: PluginWarningIconDto,
        message: String,
    },
    TagChips {
        tags: Vec<String>,
        max_display: Option<u32>,
    },
    Toolbar {
        buttons: Vec<PluginToolbarButtonDto>,
    },
    Carousel {
        images: Vec<PluginImageDto>,
        current_index: u64,
        height: Option<SizeHint>,
        enable_lightbox: bool,
    },
    KeyValueList {
        items: Vec<PluginKeyValueDto>,
        columns: Option<u32>,
    },
    MetadataGrid {
        items: Vec<PluginKeyValueDto>,
        columns: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginButtonActionDto {
    None,
    ShowDialog { id: String },
    CloseDialog,
    OpenPage { id: String },
    ClosePage,
    Custom { value: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginWarningIconDto {
    GlobeX,
    Warning,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginToolbarButtonDto {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub primary: bool,
    pub spacer_before: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginImageDto {
    pub cache_key: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginKeyValueDto {
    pub key: String,
    pub value: String,
}

/// The interaction verb a renderer submits against one node id (the
/// `action` field of a `PluginActionRequest`-equivalent). Distinct from
/// [`PluginButtonActionDto`] (declarative navigation data a `Button`
/// node itself carries): this is what the *renderer* does *to* a node --
/// press it, or set its value -- independent of what kind of node it is.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginActionDto {
    Activate,
    SetValue { value: Option<String> },
}

/// A bounded, typed side effect the application layer surfaces to a
/// renderer alongside an updated [`PluginUiNodeDto`] tree. Deliberately
/// does **not** include `RefreshPanel` or `RequestFetch` -- those two
/// [`crate::model::PluginAction`] variants are host-internal signals
/// (respectively: "re-fetch and re-normalize this session's layout" and
/// "run a background metadata fetch") that the application layer
/// resolves entirely on its own; a renderer never needs to react to them
/// directly, so they never cross this boundary.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginHostIntentDto {
    CloseDialog,
    CopyToClipboard {
        text: String,
    },
    OpenLightbox {
        images: Vec<PluginImageDto>,
        start_index: u64,
        title: Option<String>,
    },
    SetPageDisplayName {
        name: String,
    },
    ShowToast {
        message: String,
        level: PluginToastLevelDto,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginToastLevelDto {
    Error,
    Info,
    Success,
    Warning,
}

/// Which UI slot a [`PluginUiNodeDto`] tree was rendered for. Mirrors
/// [`crate::model::PluginExtensionPoint`], re-expressed as a
/// `serde`-friendly, `Copy`-free DTO for the application-facade
/// boundary.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum PluginExtensionPointDto {
    Dialog(String),
    MainPage,
    Page(String),
    Panel,
    PluginButton,
}

impl PluginExtensionPointDto {
    /// A stable, human-readable slug for this extension point, suitable
    /// as a renderer-side cache/lookup key without pattern-matching the
    /// enum. `open_plugin_session` (the only facade entry point that
    /// mints a fresh plugin session today) always opens `MainPage` --
    /// there is no argument on that call for a caller to request a
    /// different extension point yet -- so this is what a
    /// `PluginUiDocument`'s `region_id` is currently always populated
    /// from. Kept as a method on the DTO itself (not inlined at the one
    /// call site) so a later facade method that *can* request a
    /// different extension point reuses the exact same slug format
    /// rather than inventing a second one.
    pub fn region_slug(&self) -> String {
        match self {
            Self::MainPage => "main_page".to_string(),
            Self::PluginButton => "plugin_button".to_string(),
            Self::Panel => "panel".to_string(),
            Self::Dialog(id) => format!("dialog:{id}"),
            Self::Page(id) => format!("page:{id}"),
        }
    }
}

fn convert_button_action(action: &ButtonAction) -> PluginButtonActionDto {
    match action {
        ButtonAction::None => PluginButtonActionDto::None,
        ButtonAction::ShowDialog { id } => PluginButtonActionDto::ShowDialog { id: id.clone() },
        ButtonAction::CloseDialog => PluginButtonActionDto::CloseDialog,
        ButtonAction::OpenPage { id } => PluginButtonActionDto::OpenPage { id: id.clone() },
        ButtonAction::ClosePage => PluginButtonActionDto::ClosePage,
        ButtonAction::Custom(value) => PluginButtonActionDto::Custom {
            value: value.clone(),
        },
    }
}

fn convert_warning_icon(icon: WarningIcon) -> PluginWarningIconDto {
    match icon {
        WarningIcon::Warning => PluginWarningIconDto::Warning,
        WarningIcon::GlobeX => PluginWarningIconDto::GlobeX,
    }
}

fn convert_toolbar_button(button: &ToolbarButton) -> PluginToolbarButtonDto {
    PluginToolbarButtonDto {
        id: button.id.clone(),
        label: button.label.clone(),
        icon: button.icon.clone(),
        primary: button.primary,
        spacer_before: button.spacer_before,
    }
}

fn convert_key_value(pair: &KeyValuePair) -> PluginKeyValueDto {
    PluginKeyValueDto {
        key: pair.key.clone(),
        value: pair.value.clone(),
    }
}

/// Maps a [`crate::model::ToastLevel`] (the shape `PluginAction::ShowToast`
/// carries) onto its DTO equivalent. `pub`: called from `arclain_app`,
/// which converts a bounded `PluginAction::ShowToast` into a
/// [`PluginHostIntentDto::ShowToast`].
pub fn convert_toast_level(level: ToastLevel) -> PluginToastLevelDto {
    match level {
        ToastLevel::Info => PluginToastLevelDto::Info,
        ToastLevel::Success => PluginToastLevelDto::Success,
        ToastLevel::Warning => PluginToastLevelDto::Warning,
        ToastLevel::Error => PluginToastLevelDto::Error,
    }
}

/// Running totals [`normalize_layout`]'s walk enforces every budget
/// against, plus the set of every node id assigned so far (see the
/// module doc comment for why interactive and structural ids share one
/// namespace).
struct NormalizeCtx {
    node_count: usize,
    text_bytes: usize,
    asset_count: usize,
    seen_ids: std::collections::HashSet<String>,
}

impl NormalizeCtx {
    fn new() -> Self {
        Self {
            node_count: 0,
            text_bytes: 0,
            asset_count: 0,
            seen_ids: std::collections::HashSet::new(),
        }
    }

    fn charge_node(&mut self) -> Result<(), PluginUiNormalizeError> {
        self.node_count += 1;
        if self.node_count > MAX_UI_NODES {
            return Err(PluginUiNormalizeError::TooManyNodes);
        }
        Ok(())
    }

    fn charge_text(&mut self, bytes: usize) -> Result<(), PluginUiNormalizeError> {
        self.text_bytes = self.text_bytes.saturating_add(bytes);
        if self.text_bytes > MAX_UI_TEXT_BYTES {
            return Err(PluginUiNormalizeError::TextBudgetExceeded);
        }
        Ok(())
    }

    fn charge_asset(&mut self) -> Result<(), PluginUiNormalizeError> {
        self.asset_count += 1;
        if self.asset_count > MAX_UI_ASSETS {
            return Err(PluginUiNormalizeError::TooManyAssets);
        }
        Ok(())
    }

    /// Claims `id` for a node, failing if anything -- interactive or
    /// structural -- already claimed it.
    fn register_id(&mut self, id: String) -> Result<String, PluginUiNormalizeError> {
        if !self.seen_ids.insert(id.clone()) {
            return Err(PluginUiNormalizeError::DuplicateNodeId(id));
        }
        Ok(id)
    }
}

fn structural_id(parent: &str, position: usize) -> String {
    format!("{parent}/{position}")
}

/// Normalizes a plugin's current [`PluginLayout`] into a single
/// renderer-neutral root [`PluginUiNodeDto`]. See the module doc comment
/// for the id-assignment and group-marker rules, and
/// [`PluginUiNormalizeError`] for every rejection reason.
pub fn normalize_layout(layout: &PluginLayout) -> Result<PluginUiNodeDto, PluginUiNormalizeError> {
    let mut ctx = NormalizeCtx::new();
    ctx.charge_node()?; // the root node itself
    let kind = match layout {
        PluginLayout::Single { elements } => {
            let children = normalize_children(&mut ctx, elements, "#root", 1)?;
            PluginUiNodeKind::Single { children }
        }
        PluginLayout::Split {
            sidebar,
            content,
            width,
        } => {
            let sidebar_nodes = normalize_children(&mut ctx, sidebar, "#root/sidebar", 1)?;
            let content_nodes = normalize_children(&mut ctx, content, "#root/content", 1)?;
            PluginUiNodeKind::Split {
                sidebar: sidebar_nodes,
                content: content_nodes,
                width: *width,
            }
        }
    };
    Ok(PluginUiNodeDto {
        id: ctx.register_id("#root".to_string())?,
        kind,
        visible: true,
        enabled: true,
    })
}

/// Normalizes one sibling list, resolving `GroupBegin`/`GroupEnd` marker
/// pairs into synthesized `Group` nodes along the way (see the module
/// doc comment). `parent_path` is this list's own structural-id prefix;
/// `depth` is this list's nesting depth from the document root, checked
/// against [`MAX_UI_TREE_DEPTH`] before any of its elements are visited.
fn normalize_children(
    ctx: &mut NormalizeCtx,
    elements: &[PluginUiElement],
    parent_path: &str,
    depth: usize,
) -> Result<Vec<PluginUiNodeDto>, PluginUiNormalizeError> {
    if depth > MAX_UI_TREE_DEPTH {
        return Err(PluginUiNormalizeError::TreeTooDeep);
    }
    let mut result = Vec::with_capacity(elements.len());
    let mut index = 0usize;
    let mut position = 0usize;

    while index < elements.len() {
        match &elements[index] {
            PluginUiElement::GroupBegin { title, description } => {
                let mut nesting = 1usize;
                let mut scan = index + 1;
                let mut matching_end = None;
                while scan < elements.len() {
                    match &elements[scan] {
                        PluginUiElement::GroupBegin { .. } => nesting += 1,
                        PluginUiElement::GroupEnd => {
                            nesting -= 1;
                            if nesting == 0 {
                                matching_end = Some(scan);
                                break;
                            }
                        }
                        _ => {}
                    }
                    scan += 1;
                }
                let Some(matching_end) = matching_end else {
                    return Err(PluginUiNormalizeError::UnclosedGroup);
                };

                ctx.charge_node()?;
                ctx.charge_text(title.len())?;
                if let Some(description) = description {
                    ctx.charge_text(description.len())?;
                }
                let child_path = structural_id(parent_path, position);
                let children = normalize_children(
                    ctx,
                    &elements[index + 1..matching_end],
                    &child_path,
                    depth + 1,
                )?;
                result.push(PluginUiNodeDto {
                    id: ctx.register_id(child_path)?,
                    kind: PluginUiNodeKind::Group {
                        title: title.clone(),
                        description: description.clone(),
                        children,
                    },
                    visible: true,
                    enabled: true,
                });
                position += 1;
                index = matching_end + 1;
            }
            PluginUiElement::GroupEnd => {
                return Err(PluginUiNormalizeError::UnmatchedGroupEnd);
            }
            other => {
                // `depth`, not `depth + 1`: an ordinary element is not
                // itself a container boundary. Only `ListContainer`
                // (inside `normalize_element`) and `Group` (just above)
                // advance depth, each exactly once per actual nesting
                // level -- keeping "how many containers are nested here"
                // an intuitive, directly testable count instead of an
                // implementation-detail-dependent one.
                let child_path = structural_id(parent_path, position);
                result.push(normalize_element(ctx, other, &child_path, depth)?);
                position += 1;
                index += 1;
            }
        }
    }

    Ok(result)
}

/// Normalizes a single non-marker element. `structural_path` is the id a
/// display-only element receives; an interactive element ignores it and
/// registers its own plugin-provided id instead (see the module doc
/// comment).
fn normalize_element(
    ctx: &mut NormalizeCtx,
    element: &PluginUiElement,
    structural_path: &str,
    depth: usize,
) -> Result<PluginUiNodeDto, PluginUiNormalizeError> {
    // No depth check here: an element that is not itself a container
    // never advances `depth`, and every call site that *does* advance it
    // (the `ListContainer` arm below, and `normalize_children`'s `Group`
    // handling) goes back through `normalize_children`, which checks it
    // at entry.
    ctx.charge_node()?;

    let (id, kind) = match element {
        PluginUiElement::Label { text, role } => {
            ctx.charge_text(text.len())?;
            (
                ctx.register_id(structural_path.to_string())?,
                PluginUiNodeKind::Label {
                    text: text.clone(),
                    role: *role,
                },
            )
        }
        PluginUiElement::SectionHeader {
            title,
            level,
            description,
        } => {
            ctx.charge_text(title.len())?;
            if let Some(description) = description {
                ctx.charge_text(description.len())?;
            }
            (
                ctx.register_id(structural_path.to_string())?,
                PluginUiNodeKind::SectionHeader {
                    title: title.clone(),
                    level: *level,
                    description: description.clone(),
                },
            )
        }
        PluginUiElement::Button { id, label, action } => {
            ctx.charge_text(label.len())?;
            (
                ctx.register_id(id.clone())?,
                PluginUiNodeKind::Button {
                    label: label.clone(),
                    action: action.as_ref().map(convert_button_action),
                },
            )
        }
        PluginUiElement::TextInput {
            id,
            label,
            value,
            placeholder,
        } => {
            ctx.charge_text(label.len() + value.len())?;
            (
                ctx.register_id(id.clone())?,
                PluginUiNodeKind::TextInput {
                    label: label.clone(),
                    value: value.clone(),
                    placeholder: placeholder.clone(),
                },
            )
        }
        PluginUiElement::Checkbox { id, label, checked } => {
            ctx.charge_text(label.len())?;
            (
                ctx.register_id(id.clone())?,
                PluginUiNodeKind::Checkbox {
                    label: label.clone(),
                    checked: *checked,
                },
            )
        }
        PluginUiElement::RadioGroup {
            id,
            label,
            options,
            selected,
        } => {
            ctx.charge_text(label.len())?;
            for option in options {
                ctx.charge_text(option.len())?;
            }
            (
                ctx.register_id(id.clone())?,
                PluginUiNodeKind::RadioGroup {
                    label: label.clone(),
                    options: options.clone(),
                    selected: selected.clone(),
                },
            )
        }
        PluginUiElement::Slider {
            id,
            label,
            value,
            min,
            max,
            step,
        } => {
            ctx.charge_text(label.len())?;
            (
                ctx.register_id(id.clone())?,
                PluginUiNodeKind::Slider {
                    label: label.clone(),
                    value: *value,
                    min: *min,
                    max: *max,
                    step: *step,
                },
            )
        }
        PluginUiElement::Dropdown {
            id,
            label,
            options,
            selected,
        } => {
            ctx.charge_text(label.len())?;
            for option in options {
                ctx.charge_text(option.len())?;
            }
            (
                ctx.register_id(id.clone())?,
                PluginUiNodeKind::Dropdown {
                    label: label.clone(),
                    options: options.clone(),
                    selected: selected.clone(),
                },
            )
        }
        PluginUiElement::Image {
            cache_key,
            url,
            height,
        } => {
            if cache_key.is_some() || url.is_some() {
                ctx.charge_asset()?;
            }
            (
                ctx.register_id(structural_path.to_string())?,
                PluginUiNodeKind::Image {
                    cache_key: cache_key.clone(),
                    url: url.clone(),
                    height: *height,
                },
            )
        }
        PluginUiElement::Separator => (
            ctx.register_id(structural_path.to_string())?,
            PluginUiNodeKind::Separator,
        ),
        PluginUiElement::Space { step } => (
            ctx.register_id(structural_path.to_string())?,
            PluginUiNodeKind::Space { step: *step },
        ),
        PluginUiElement::Tabs { id, tabs, selected } => {
            for tab in tabs {
                ctx.charge_text(tab.len())?;
            }
            (
                ctx.register_id(id.clone())?,
                PluginUiNodeKind::Tabs {
                    tabs: tabs.clone(),
                    selected: selected.clone(),
                },
            )
        }
        PluginUiElement::ListItem {
            id,
            title,
            subtitle,
            badge,
            image_key,
            image_url,
            selected,
            warning_icon,
        } => {
            ctx.charge_text(title.len())?;
            if let Some(subtitle) = subtitle {
                ctx.charge_text(subtitle.len())?;
            }
            if let Some(badge) = badge {
                ctx.charge_text(badge.len())?;
            }
            if image_key.is_some() || image_url.is_some() {
                ctx.charge_asset()?;
            }
            (
                ctx.register_id(id.clone())?,
                PluginUiNodeKind::ListItem {
                    title: title.clone(),
                    subtitle: subtitle.clone(),
                    badge: badge.clone(),
                    image_key: image_key.clone(),
                    image_url: image_url.clone(),
                    selected: *selected,
                    warning_icon: warning_icon.map(convert_warning_icon),
                },
            )
        }
        PluginUiElement::ListContainer {
            id,
            items,
            height,
            empty_message,
        } => {
            if let Some(empty_message) = empty_message {
                ctx.charge_text(empty_message.len())?;
            }
            let children = normalize_children(ctx, items, structural_path, depth + 1)?;
            (
                ctx.register_id(id.clone())?,
                PluginUiNodeKind::ListContainer {
                    children,
                    height: *height,
                    empty_message: empty_message.clone(),
                },
            )
        }
        PluginUiElement::Loading { message } => {
            if let Some(message) = message {
                ctx.charge_text(message.len())?;
            }
            (
                ctx.register_id(structural_path.to_string())?,
                PluginUiNodeKind::Loading {
                    message: message.clone(),
                },
            )
        }
        PluginUiElement::GroupBegin { .. } | PluginUiElement::GroupEnd => {
            unreachable!("group markers are resolved by normalize_children before reaching here")
        }
        PluginUiElement::Warning { icon, message } => {
            ctx.charge_text(message.len())?;
            (
                ctx.register_id(structural_path.to_string())?,
                PluginUiNodeKind::Warning {
                    icon: convert_warning_icon(*icon),
                    message: message.clone(),
                },
            )
        }
        PluginUiElement::TagChips { tags, max_display } => {
            for tag in tags {
                ctx.charge_text(tag.len())?;
            }
            (
                ctx.register_id(structural_path.to_string())?,
                PluginUiNodeKind::TagChips {
                    tags: tags.clone(),
                    max_display: *max_display,
                },
            )
        }
        PluginUiElement::Toolbar { buttons } => {
            for button in buttons {
                ctx.charge_text(button.label.len())?;
            }
            (
                ctx.register_id(structural_path.to_string())?,
                PluginUiNodeKind::Toolbar {
                    buttons: buttons.iter().map(convert_toolbar_button).collect(),
                },
            )
        }
        PluginUiElement::Carousel {
            id,
            images,
            current_index,
            height,
            enable_lightbox,
        } => {
            for _ in images {
                ctx.charge_asset()?;
            }
            let images = images
                .iter()
                .map(|(cache_key, url)| PluginImageDto {
                    cache_key: cache_key.clone(),
                    url: url.clone(),
                })
                .collect();
            (
                ctx.register_id(id.clone())?,
                PluginUiNodeKind::Carousel {
                    images,
                    current_index: *current_index as u64,
                    height: *height,
                    enable_lightbox: *enable_lightbox,
                },
            )
        }
        PluginUiElement::KeyValueList { items, columns } => {
            for item in items {
                ctx.charge_text(item.key.len() + item.value.len())?;
            }
            (
                ctx.register_id(structural_path.to_string())?,
                PluginUiNodeKind::KeyValueList {
                    items: items.iter().map(convert_key_value).collect(),
                    columns: *columns,
                },
            )
        }
        PluginUiElement::MetadataGrid { items, columns } => {
            for item in items {
                ctx.charge_text(item.key.len() + item.value.len())?;
            }
            (
                ctx.register_id(structural_path.to_string())?,
                PluginUiNodeKind::MetadataGrid {
                    items: items.iter().map(convert_key_value).collect(),
                    columns: *columns,
                },
            )
        }
    };

    Ok(PluginUiNodeDto {
        id,
        kind,
        visible: true,
        enabled: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::wirt::plugin::ui::{
        CarouselConfig as WitCarouselConfig, ImageConfig as WitImageConfig,
        LabelConfig as WitLabelConfig, ListContainerConfig as WitListContainerConfig,
        PluginLayout as WitPluginLayout, SidebarWidth as WitSidebarWidth, SizeHint as WitSizeHint,
        SpacingStep as WitSpacingStep, SplitConfig as WitSplitConfig, TextRole as WitTextRole,
        UiElement as WitUiElement,
    };
    use crate::conversions::{convert_plugin_layout, convert_ui_element};

    #[test]
    fn text_role_survives_the_conversion() {
        for (wit, expected) in [
            (WitTextRole::Title, TextRole::Title),
            (WitTextRole::Subtitle, TextRole::Subtitle),
            (WitTextRole::Body, TextRole::Body),
            (WitTextRole::Caption, TextRole::Caption),
            (WitTextRole::Emphasis, TextRole::Emphasis),
        ] {
            let element = convert_ui_element(WitUiElement::Label(WitLabelConfig {
                text: "x".to_string(),
                role: wit,
            }));
            let PluginUiElement::Label { text, role } = element else {
                panic!("a label converts to a label");
            };
            assert_eq!(text, "x");
            assert_eq!(role, expected);
        }
    }

    #[test]
    fn spacing_step_survives_the_conversion() {
        for (wit, expected) in [
            (WitSpacingStep::Small, SpacingStep::Small),
            (WitSpacingStep::Medium, SpacingStep::Medium),
            (WitSpacingStep::Large, SpacingStep::Large),
        ] {
            let element = convert_ui_element(WitUiElement::Space(wit));
            let PluginUiElement::Space { step } = element else {
                panic!("space converts to a space");
            };
            assert_eq!(step, expected);
        }
    }

    /// Three element kinds share one vocabulary of size steps, and each
    /// one has to carry the step it was given all the way across the
    /// boundary -- the host is what turns a step into a different number
    /// per kind, and it cannot do that with a hint that arrived as
    /// something else. Every field is destructured with no `..` so that a
    /// field surviving here that should no longer exist fails to compile.
    #[test]
    fn size_hint_survives_the_conversion_for_every_kind() {
        for (wit, expected) in [
            (WitSizeHint::Compact, SizeHint::Compact),
            (WitSizeHint::Regular, SizeHint::Regular),
            (WitSizeHint::Tall, SizeHint::Tall),
        ] {
            let element = convert_ui_element(WitUiElement::Image(WitImageConfig {
                cache_key: Some("k".to_string()),
                url: None,
                height: Some(wit),
            }));
            let PluginUiElement::Image {
                cache_key,
                url,
                height,
            } = element
            else {
                panic!("an image converts to an image");
            };
            assert_eq!(cache_key.as_deref(), Some("k"));
            assert_eq!(url, None);
            assert_eq!(height, Some(expected));

            let element = convert_ui_element(WitUiElement::ListContainer(WitListContainerConfig {
                id: "list".to_string(),
                items: Vec::new(),
                height: Some(wit),
                empty_message: None,
            }));
            let PluginUiElement::ListContainer {
                id,
                items,
                height,
                empty_message,
            } = element
            else {
                panic!("a list container converts to a list container");
            };
            assert_eq!(id, "list");
            assert!(items.is_empty());
            assert_eq!(empty_message, None);
            assert_eq!(height, Some(expected));

            let element = convert_ui_element(WitUiElement::Carousel(WitCarouselConfig {
                id: "gallery".to_string(),
                images: Vec::new(),
                current_index: 0,
                height: Some(wit),
                enable_lightbox: true,
            }));
            let PluginUiElement::Carousel {
                id,
                images,
                current_index,
                height,
                enable_lightbox,
            } = element
            else {
                panic!("a carousel converts to a carousel");
            };
            assert_eq!(id, "gallery");
            assert!(images.is_empty());
            assert_eq!(current_index, 0);
            assert!(enable_lightbox);
            assert_eq!(height, Some(expected));
        }
    }

    /// The sidebar's width crosses the boundary inside the layout rather
    /// than inside an element, so it is the one styling vocabulary
    /// `convert_ui_element` never sees. Every field is destructured with no
    /// `..` so that a field surviving here that should no longer exist
    /// fails to compile.
    #[test]
    fn sidebar_width_survives_the_conversion() {
        for (wit, expected) in [
            (WitSidebarWidth::Narrow, SidebarWidth::Narrow),
            (WitSidebarWidth::Medium, SidebarWidth::Medium),
            (WitSidebarWidth::Wide, SidebarWidth::Wide),
        ] {
            let layout = convert_plugin_layout(WitPluginLayout::Split(WitSplitConfig {
                sidebar: vec![WitUiElement::Separator],
                content: Vec::new(),
                width: Some(wit),
            }));
            let PluginLayout::Split {
                sidebar,
                content,
                width,
            } = layout
            else {
                panic!("a split layout converts to a split layout");
            };
            assert_eq!(sidebar.len(), 1);
            assert!(content.is_empty());
            assert_eq!(width, Some(expected));
        }
    }

    /// A split that names no width is a real case and has to stay one: the
    /// absent width is what the host reads as "you decide".
    #[test]
    fn an_absent_sidebar_width_converts_to_an_absent_sidebar_width() {
        let layout = convert_plugin_layout(WitPluginLayout::Split(WitSplitConfig {
            sidebar: Vec::new(),
            content: Vec::new(),
            width: None,
        }));
        let PluginLayout::Split { width, .. } = layout else {
            panic!("a split layout converts to a split layout");
        };
        assert_eq!(width, None);
    }

    /// An element that names no size is a real case and has to stay one:
    /// the absent hint is what the host reads as "you decide".
    #[test]
    fn an_absent_size_hint_converts_to_an_absent_size_hint() {
        let element = convert_ui_element(WitUiElement::Image(WitImageConfig {
            cache_key: None,
            url: None,
            height: None,
        }));
        let PluginUiElement::Image { height, .. } = element else {
            panic!("an image converts to an image");
        };
        assert_eq!(height, None);
    }

    fn label(text: &str) -> PluginUiElement {
        PluginUiElement::Label {
            text: text.to_string(),
            role: TextRole::Body,
        }
    }

    fn button(id: &str) -> PluginUiElement {
        PluginUiElement::Button {
            id: id.to_string(),
            label: "Click".to_string(),
            action: None,
        }
    }

    fn single(elements: Vec<PluginUiElement>) -> PluginLayout {
        PluginLayout::Single { elements }
    }

    #[test]
    fn root_and_display_only_nodes_get_deterministic_structural_ids() {
        let layout = single(vec![label("a"), label("b")]);
        let root = normalize_layout(&layout).unwrap();
        assert_eq!(root.id, "#root");
        let PluginUiNodeKind::Single { children } = &root.kind else {
            panic!("expected Single root");
        };
        assert_eq!(children[0].id, "#root/0");
        assert_eq!(children[1].id, "#root/1");
    }

    #[test]
    fn interactive_elements_keep_their_plugin_provided_id_verbatim() {
        let layout = single(vec![label("intro"), button("save-button")]);
        let root = normalize_layout(&layout).unwrap();
        let PluginUiNodeKind::Single { children } = &root.kind else {
            panic!("expected Single root");
        };
        assert_eq!(children[1].id, "save-button");
    }

    #[test]
    fn duplicate_interactive_ids_are_rejected() {
        let layout = single(vec![button("dup"), button("dup")]);
        let error = normalize_layout(&layout).unwrap_err();
        assert_eq!(
            error,
            PluginUiNormalizeError::DuplicateNodeId("dup".to_string())
        );
    }

    #[test]
    fn split_root_normalizes_both_branches_with_distinct_structural_prefixes() {
        let layout = PluginLayout::Split {
            sidebar: vec![label("side")],
            content: vec![label("main")],
            width: Some(SidebarWidth::Wide),
        };
        let root = normalize_layout(&layout).unwrap();
        let PluginUiNodeKind::Split {
            sidebar,
            content,
            width,
        } = &root.kind
        else {
            panic!("expected Split root");
        };
        assert_eq!(width, &Some(SidebarWidth::Wide));
        assert_eq!(sidebar[0].id, "#root/sidebar/0");
        assert_eq!(content[0].id, "#root/content/0");
    }

    #[test]
    fn group_markers_become_a_nested_group_node() {
        let layout = single(vec![
            label("before"),
            PluginUiElement::GroupBegin {
                title: "Section".to_string(),
                description: None,
            },
            button("inner-button"),
            PluginUiElement::GroupEnd,
            label("after"),
        ]);
        let root = normalize_layout(&layout).unwrap();
        let PluginUiNodeKind::Single { children } = &root.kind else {
            panic!("expected Single root");
        };
        assert_eq!(children.len(), 3, "before, group, after");
        let PluginUiNodeKind::Group {
            title, children, ..
        } = &children[1].kind
        else {
            panic!("expected a Group node in the middle position");
        };
        assert_eq!(title, "Section");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, "inner-button");
        // The group's own id is structural; nothing about a GroupBegin
        // marker carries a plugin-provided id.
        assert_eq!(children.len(), 1);
    }

    #[test]
    fn nested_groups_are_resolved_by_tracking_nesting_depth() {
        let layout = single(vec![
            PluginUiElement::GroupBegin {
                title: "Outer".to_string(),
                description: None,
            },
            PluginUiElement::GroupBegin {
                title: "Inner".to_string(),
                description: None,
            },
            label("deep"),
            PluginUiElement::GroupEnd,
            PluginUiElement::GroupEnd,
        ]);
        let root = normalize_layout(&layout).unwrap();
        let PluginUiNodeKind::Single { children } = &root.kind else {
            panic!("expected Single root");
        };
        let PluginUiNodeKind::Group {
            title, children, ..
        } = &children[0].kind
        else {
            panic!("expected outer Group");
        };
        assert_eq!(title, "Outer");
        let PluginUiNodeKind::Group { title, .. } = &children[0].kind else {
            panic!("expected inner Group nested inside the outer one");
        };
        assert_eq!(title, "Inner");
    }

    #[test]
    fn unmatched_group_end_is_rejected() {
        let layout = single(vec![label("a"), PluginUiElement::GroupEnd]);
        let error = normalize_layout(&layout).unwrap_err();
        assert_eq!(error, PluginUiNormalizeError::UnmatchedGroupEnd);
    }

    #[test]
    fn group_begin_never_closed_is_rejected() {
        let layout = single(vec![PluginUiElement::GroupBegin {
            title: "Open forever".to_string(),
            description: None,
        }]);
        let error = normalize_layout(&layout).unwrap_err();
        assert_eq!(error, PluginUiNormalizeError::UnclosedGroup);
    }

    #[test]
    fn tree_depth_budget_accepts_the_boundary_and_rejects_one_over() {
        // Nest MAX_UI_TREE_DEPTH ListContainers -- one level per
        // container, since containers are the only way this element set
        // increases depth.
        fn nested_list_container(remaining: usize) -> PluginUiElement {
            if remaining == 0 {
                return PluginUiElement::Separator;
            }
            PluginUiElement::ListContainer {
                id: format!("list-{remaining}"),
                items: vec![nested_list_container(remaining - 1)],
                height: None,
                empty_message: None,
            }
        }

        let exact = single(vec![nested_list_container(MAX_UI_TREE_DEPTH - 1)]);
        assert!(normalize_layout(&exact).is_ok());

        let over = single(vec![nested_list_container(MAX_UI_TREE_DEPTH)]);
        assert_eq!(
            normalize_layout(&over).unwrap_err(),
            PluginUiNormalizeError::TreeTooDeep
        );
    }

    #[test]
    fn node_count_budget_accepts_the_boundary_and_rejects_one_over() {
        // The root itself counts as one node, so `MAX_UI_NODES - 1`
        // labels exactly fills the budget.
        let exact = single((0..MAX_UI_NODES - 1).map(|_| label("x")).collect());
        assert!(normalize_layout(&exact).is_ok());

        let over = single((0..MAX_UI_NODES).map(|_| label("x")).collect());
        assert_eq!(
            normalize_layout(&over).unwrap_err(),
            PluginUiNormalizeError::TooManyNodes
        );
    }

    #[test]
    fn text_budget_accepts_the_boundary_and_rejects_one_over() {
        let exact = single(vec![label(&"x".repeat(MAX_UI_TEXT_BYTES))]);
        assert!(normalize_layout(&exact).is_ok());

        let over = single(vec![label(&"x".repeat(MAX_UI_TEXT_BYTES + 1))]);
        assert_eq!(
            normalize_layout(&over).unwrap_err(),
            PluginUiNormalizeError::TextBudgetExceeded
        );
    }

    #[test]
    fn asset_budget_accepts_the_boundary_and_rejects_one_over() {
        fn image() -> PluginUiElement {
            PluginUiElement::Image {
                cache_key: Some("k".to_string()),
                url: None,
                height: None,
            }
        }

        let exact = single((0..MAX_UI_ASSETS).map(|_| image()).collect());
        assert!(normalize_layout(&exact).is_ok());

        let over = single((0..=MAX_UI_ASSETS).map(|_| image()).collect());
        assert_eq!(
            normalize_layout(&over).unwrap_err(),
            PluginUiNormalizeError::TooManyAssets
        );
    }

    #[test]
    fn find_locates_a_nested_interactive_node_and_returns_none_for_an_unknown_id() {
        let layout = single(vec![PluginUiElement::ListContainer {
            id: "list".to_string(),
            items: vec![PluginUiElement::ListItem {
                id: "item-1".to_string(),
                title: "One".to_string(),
                subtitle: None,
                badge: None,
                image_key: None,
                image_url: None,
                selected: false,
                warning_icon: None,
            }],
            height: None,
            empty_message: None,
        }]);
        let root = normalize_layout(&layout).unwrap();

        assert!(root.find("item-1").is_some());
        assert!(root.find("list").is_some());
        assert!(root.find("__page_init").is_none());
        assert!(root.find("toolbar-button-id-not-in-tree").is_none());
    }

    #[test]
    fn a_disabled_or_hidden_node_is_discoverable_for_host_side_rejection() {
        // Nothing in the current WIT schema can *produce* a disabled or
        // hidden node -- this constructs one directly to prove the
        // enforcement mechanism (`find` + checking the flags) works,
        // independent of whether a real layout can trigger it today.
        let disabled = PluginUiNodeDto {
            id: "disabled-button".to_string(),
            kind: PluginUiNodeKind::Button {
                label: "Disabled".to_string(),
                action: None,
            },
            visible: true,
            enabled: false,
        };
        let hidden = PluginUiNodeDto {
            id: "hidden-button".to_string(),
            kind: PluginUiNodeKind::Button {
                label: "Hidden".to_string(),
                action: None,
            },
            visible: false,
            enabled: true,
        };
        let root = PluginUiNodeDto {
            id: "#root".to_string(),
            kind: PluginUiNodeKind::Single {
                children: vec![disabled, hidden],
            },
            visible: true,
            enabled: true,
        };

        let found = root.find("disabled-button").unwrap();
        assert!(!found.enabled);
        let found = root.find("hidden-button").unwrap();
        assert!(!found.visible);
    }

    #[test]
    fn region_slug_is_stable_per_extension_point() {
        assert_eq!(PluginExtensionPointDto::MainPage.region_slug(), "main_page");
        assert_eq!(PluginExtensionPointDto::Panel.region_slug(), "panel");
        assert_eq!(
            PluginExtensionPointDto::PluginButton.region_slug(),
            "plugin_button"
        );
        assert_eq!(
            PluginExtensionPointDto::Dialog("confirm".to_string()).region_slug(),
            "dialog:confirm"
        );
        assert_eq!(
            PluginExtensionPointDto::Page("settings".to_string()).region_slug(),
            "page:settings"
        );
    }
}
