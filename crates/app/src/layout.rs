//! The application's own chrome-layout surface: the toolbar buttons,
//! context-menu entries, tools-dialog entries and info-panel sections a
//! user arranges, plus the display options that sit beside them.
//!
//! This module holds the DTOs, the conversions to and from
//! `arclain_core`'s stored shapes, and the pure validation the write path
//! runs; `crate::runtime::layout_ops` is the `AppRuntime`-touching
//! execution layer, and `crate::runtime`'s own `impl ArclainApp` exposes
//! the thin dispatch wrappers -- the same three-way split
//! `crate::organization`/`runtime::organization_ops` uses.
//!
//! ## Why a mirror at all
//!
//! `arclain_core::{UiItem, UiRegion, ActionType, DisplayMode}` are
//! re-exports of `arclain_db`'s own row shapes. A frontend that named
//! them would be depending on the storage layer's vocabulary to draw its
//! own chrome; these DTOs are that vocabulary restated as the
//! application's, so `arclain_ui` (and later a Flutter bridge) needs
//! nothing but this crate.
//!
//! The mirrors are deliberately structural rather than tolerant: every
//! `From` impl below matches exhaustively and destructures without `..`,
//! so a field or variant added upstream fails to compile here until this
//! mirror carries it too.
//!
//! ## Naming collision worth knowing about
//!
//! [`UiRegionDto`] is *not* related to `crate::plugins::
//! PluginUiDocument::region_id`. This is a customizable region of the
//! application's own chrome; that is a plugin-document slot. Same English
//! word, different concept -- see that field's own doc comment.

use arclain_core::{ActionType, DisplayMode, UiItem, UiRegion};

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};

/// The longest any one text field of a [`UiItemDto`] may be on the way in.
///
/// Not defensive noise: a toolbar item's `label` can originate in a
/// plugin (the layout editor offers every enabled plugin's buttons as
/// arrangeable items, labelled from the plugin's own UI declaration), and
/// a plugin is untrusted WASM. Without a bound, one plugin could persist
/// a megabyte of label text into the user's configuration database and
/// have every later layout read carry it forever. A kilobyte is orders of
/// magnitude more than any real id, label, icon name or action payload
/// needs.
pub const MAX_UI_ITEM_TEXT_BYTES: usize = 1024;

/// The most items one [`ArclainApp::save_ui_items`](crate::ArclainApp::save_ui_items)
/// call may write for a region. The seeded regions hold a dozen-odd
/// entries each and plugins add a handful; this exists so a malformed
/// caller cannot ask for an unbounded write loop.
pub const MAX_UI_ITEMS_PER_REGION: usize = 512;

/// The widest a stored panel width may be, in logical pixels. Guards the
/// stored value against a non-finite or absurd number that would later
/// come back out of the database and drive a layout computation -- the
/// application's own sliders never approach it.
pub const MAX_UI_PANEL_WIDTH_PX: f32 = 10_000.0;

// ============================================================================
// Enum mirrors. Variant order follows the source declarations so the
// correspondence reads top to bottom.
// ============================================================================

/// One customizable region of the application's chrome. Mirrors
/// `arclain_core::UiRegion` variant for variant.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UiRegionDto {
    Toolbar,
    ContextMenu,
    ToolsDialog,
    InfoPanel,
}

impl From<UiRegion> for UiRegionDto {
    fn from(region: UiRegion) -> Self {
        match region {
            UiRegion::Toolbar => Self::Toolbar,
            UiRegion::ContextMenu => Self::ContextMenu,
            UiRegion::ToolsDialog => Self::ToolsDialog,
            UiRegion::InfoPanel => Self::InfoPanel,
        }
    }
}

impl From<UiRegionDto> for UiRegion {
    fn from(region: UiRegionDto) -> Self {
        match region {
            UiRegionDto::Toolbar => Self::Toolbar,
            UiRegionDto::ContextMenu => Self::ContextMenu,
            UiRegionDto::ToolsDialog => Self::ToolsDialog,
            UiRegionDto::InfoPanel => Self::InfoPanel,
        }
    }
}

/// What a chrome item does when activated. Mirrors
/// `arclain_core::ActionType` variant for variant, including which
/// variant is the default (`Builtin`: an item whose behavior the
/// application itself supplies).
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum UiActionTypeDto {
    #[default]
    Builtin,
    Plugin,
    Custom,
}

impl From<ActionType> for UiActionTypeDto {
    fn from(action_type: ActionType) -> Self {
        match action_type {
            ActionType::Builtin => Self::Builtin,
            ActionType::Plugin => Self::Plugin,
            ActionType::Custom => Self::Custom,
        }
    }
}

impl From<UiActionTypeDto> for ActionType {
    fn from(action_type: UiActionTypeDto) -> Self {
        match action_type {
            UiActionTypeDto::Builtin => Self::Builtin,
            UiActionTypeDto::Plugin => Self::Plugin,
            UiActionTypeDto::Custom => Self::Custom,
        }
    }
}

/// How a chrome item renders itself. Mirrors
/// `arclain_core::DisplayMode` variant for variant, including which
/// variant is the default.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum UiDisplayModeDto {
    #[default]
    IconAndText,
    IconOnly,
    TextOnly,
}

impl From<DisplayMode> for UiDisplayModeDto {
    fn from(display_mode: DisplayMode) -> Self {
        match display_mode {
            DisplayMode::IconAndText => Self::IconAndText,
            DisplayMode::IconOnly => Self::IconOnly,
            DisplayMode::TextOnly => Self::TextOnly,
        }
    }
}

impl From<UiDisplayModeDto> for DisplayMode {
    fn from(display_mode: UiDisplayModeDto) -> Self {
        match display_mode {
            UiDisplayModeDto::IconAndText => Self::IconAndText,
            UiDisplayModeDto::IconOnly => Self::IconOnly,
            UiDisplayModeDto::TextOnly => Self::TextOnly,
        }
    }
}

/// Which archive view the browser opens on by default.
///
/// Stored as free text (`"list"`/`"grid"`), read back as this closed set:
/// the application only ever writes those two tokens, and a frontend can
/// do nothing useful with a third value anyway. Anything else on disk
/// reads as [`UiViewModeDto::List`], which is also the seeded default.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum UiViewModeDto {
    #[default]
    List,
    Grid,
}

impl UiViewModeDto {
    /// The token this mode is stored as.
    pub const fn as_stored(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Grid => "grid",
        }
    }

    /// Parses a stored token. Only `"grid"` selects `Grid`; every other
    /// value (including an absent option) is `List` -- see the type's own
    /// doc comment.
    pub fn from_stored(value: &str) -> Self {
        match value {
            "grid" => Self::Grid,
            _ => Self::List,
        }
    }
}

// ============================================================================
// Item mirror.
// ============================================================================

/// One arrangeable chrome item: a toolbar button, a context-menu entry, a
/// tools-dialog entry, or an info-panel section. Mirrors
/// `arclain_core::UiItem` field for field.
///
/// `id` is the stored primary key and the value the application matches
/// its built-in behaviors on (`"toolbar.extract"`, `"info.archive"`, and
/// so on), so it is chosen by whoever creates the item rather than minted
/// here -- unlike this crate's opaque identifiers.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct UiItemDto {
    pub id: String,
    pub region: UiRegionDto,
    pub group_id: Option<String>,
    pub label: String,
    pub icon: Option<String>,
    pub visible: bool,
    pub sort_order: i32,
    pub display_mode: UiDisplayModeDto,
    pub action_type: UiActionTypeDto,
    pub action_data: Option<String>,
}

impl From<UiItem> for UiItemDto {
    fn from(item: UiItem) -> Self {
        // Destructured rather than field-accessed on purpose: a field
        // added to `UiItem` fails to compile here (and in the reverse
        // impl below) until this mirror carries it too, which is the only
        // thing that keeps "field for field" true over time.
        let UiItem {
            id,
            region,
            group_id,
            label,
            icon,
            visible,
            sort_order,
            display_mode,
            action_type,
            action_data,
        } = item;
        Self {
            id,
            region: region.into(),
            group_id,
            label,
            icon,
            visible,
            sort_order,
            display_mode: display_mode.into(),
            action_type: action_type.into(),
            action_data,
        }
    }
}

impl From<UiItemDto> for UiItem {
    fn from(item: UiItemDto) -> Self {
        let UiItemDto {
            id,
            region,
            group_id,
            label,
            icon,
            visible,
            sort_order,
            display_mode,
            action_type,
            action_data,
        } = item;
        Self {
            id,
            region: region.into(),
            group_id,
            label,
            icon,
            visible,
            sort_order,
            display_mode: display_mode.into(),
            action_type: action_type.into(),
            action_data,
        }
    }
}

// ============================================================================
// Display options.
// ============================================================================

/// The chrome display options stored alongside the items: what the
/// browser opens on, whether each side panel starts open and how wide,
/// and whether header buttons carry text labels.
///
/// A whole-value read and a whole-value write, not a patch: the settings
/// page that owns these holds all of them at once, and there is no
/// revision to check against (see
/// [`ArclainApp::save_ui_display_options`](crate::ArclainApp::save_ui_display_options)
/// for what that means for concurrent writers).
///
/// [`Default`] is the fresh-profile answer: it reproduces exactly what a
/// read of a database with none of these options set returns, so a caller
/// with no stored configuration and a caller with a freshly seeded one
/// see the same values.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct UiDisplayOptionsDto {
    pub default_view_mode: UiViewModeDto,
    pub tree_panel_visible: bool,
    pub tree_panel_width: f32,
    pub properties_panel_visible: bool,
    pub properties_panel_width: f32,
    pub show_button_labels: bool,
}

impl Default for UiDisplayOptionsDto {
    fn default() -> Self {
        Self {
            default_view_mode: UiViewModeDto::List,
            tree_panel_visible: true,
            tree_panel_width: 200.0,
            properties_panel_visible: true,
            properties_panel_width: 280.0,
            show_button_labels: false,
        }
    }
}

/// The stored option keys, in the order [`UiDisplayOptionsDto`] declares
/// its fields. Named once so the read and the write cannot disagree about
/// a key's spelling.
pub(crate) const DEFAULT_VIEW_MODE_KEY: &str = "default_view_mode";
pub(crate) const TREE_PANEL_VISIBLE_KEY: &str = "tree_panel_visible";
pub(crate) const TREE_PANEL_WIDTH_KEY: &str = "tree_panel_width";
pub(crate) const PROPERTIES_PANEL_VISIBLE_KEY: &str = "properties_panel_visible";
pub(crate) const PROPERTIES_PANEL_WIDTH_KEY: &str = "properties_panel_width";
pub(crate) const SHOW_BUTTON_LABELS_KEY: &str = "show_button_labels";

/// Parses one stored boolean option. `"true"` is true and *everything
/// else* -- including an absent option and an unparseable one -- is
/// `fallback`, matching how these flags have always been read.
pub(crate) fn stored_bool(value: Option<String>, fallback: bool) -> bool {
    match value {
        Some(value) => value == "true",
        None => fallback,
    }
}

/// Parses one stored width. An absent or unparseable value is
/// `fallback`, so a corrupted row degrades to the default rather than
/// failing a whole page load.
pub(crate) fn stored_width(value: Option<String>, fallback: f32) -> f32 {
    value
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|width| is_storable_width(*width))
        .unwrap_or(fallback)
}

fn is_storable_width(width: f32) -> bool {
    // The finiteness check is not redundant with the range check for
    // NaN's sake alone -- it says what is being asserted, and a reader
    // should not have to know that a range comparison happens to reject
    // NaN too.
    width.is_finite() && (0.0..=MAX_UI_PANEL_WIDTH_PX).contains(&width)
}

// ============================================================================
// Write-path validation.
// ============================================================================

/// Checks one batch of items destined for `region`, returning them as the
/// stored shape.
///
/// Three things are refused, all of them silent corruption if let
/// through:
///
/// * an item whose own `region` disagrees with the region being written
///   -- one region's editor writing rows into another's is never
///   intentional, and the stored row would then appear in a region whose
///   editor never offered it;
/// * an empty `id` -- the stored primary key, so a row with an empty one
///   can never be addressed again;
/// * two items sharing an `id` in one batch -- the second upsert would
///   silently overwrite the first, making the write's outcome depend on
///   list order.
///
/// Plus the bounds documented on [`MAX_UI_ITEMS_PER_REGION`] and
/// [`MAX_UI_ITEM_TEXT_BYTES`].
pub(crate) fn items_to_core(
    region: UiRegionDto,
    items: Vec<UiItemDto>,
) -> Result<Vec<UiItem>, ApplicationError> {
    if items.len() > MAX_UI_ITEMS_PER_REGION {
        return Err(invalid_input(
            "too many layout items in one save",
            format!(
                "{} items submitted, at most {MAX_UI_ITEMS_PER_REGION} are accepted",
                items.len()
            ),
            "items",
        ));
    }

    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (index, item) in items.iter().enumerate() {
        if item.region != region {
            return Err(invalid_input(
                "layout item belongs to a different region",
                format!(
                    "item {:?} at index {index} names region {:?} but is being saved into {:?}",
                    item.id, item.region, region
                ),
                format!("items[{index}].region"),
            ));
        }
        if item.id.is_empty() {
            return Err(invalid_input(
                "layout item id must not be empty",
                format!("item at index {index} has an empty id"),
                format!("items[{index}].id"),
            ));
        }
        if !seen.insert(item.id.as_str()) {
            return Err(invalid_input(
                "two layout items share one id",
                format!(
                    "id {:?} appears more than once, last at index {index}",
                    item.id
                ),
                format!("items[{index}].id"),
            ));
        }
        check_text(&item.id, index, "id")?;
        check_text(&item.label, index, "label")?;
        if let Some(group_id) = item.group_id.as_deref() {
            check_text(group_id, index, "group_id")?;
        }
        if let Some(icon) = item.icon.as_deref() {
            check_text(icon, index, "icon")?;
        }
        if let Some(action_data) = item.action_data.as_deref() {
            check_text(action_data, index, "action_data")?;
        }
    }

    Ok(items.into_iter().map(UiItem::from).collect())
}

fn check_text(value: &str, index: usize, field: &str) -> Result<(), ApplicationError> {
    if value.len() > MAX_UI_ITEM_TEXT_BYTES {
        return Err(invalid_input(
            "layout item text is too long",
            format!(
                "items[{index}].{field} is {} bytes, at most {MAX_UI_ITEM_TEXT_BYTES} are accepted",
                value.len()
            ),
            format!("items[{index}].{field}"),
        ));
    }
    Ok(())
}

/// Checks a whole display-options value before any of it is written.
/// Only the two widths can be wrong: every other field is a bool or a
/// closed enum, so the type already constrains it.
pub(crate) fn check_display_options(options: &UiDisplayOptionsDto) -> Result<(), ApplicationError> {
    for (width, field) in [
        (options.tree_panel_width, TREE_PANEL_WIDTH_KEY),
        (options.properties_panel_width, PROPERTIES_PANEL_WIDTH_KEY),
    ] {
        if !is_storable_width(width) {
            return Err(invalid_input(
                "panel width is not a usable number of pixels",
                format!(
                    "{field} is {width}, which must be finite and between 0 and \
                     {MAX_UI_PANEL_WIDTH_PX}"
                ),
                field,
            ));
        }
    }
    Ok(())
}

fn invalid_input(
    summary: &'static str,
    diagnostic: String,
    field: impl Into<String>,
) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::InvalidInput, summary)
        .with_diagnostic(diagnostic)
        .with_recoverability(Recoverability::UserAction)
        .with_field(field)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(value: &T) -> T
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        serde_json::from_str(&serde_json::to_string(value).expect("serialize"))
            .expect("deserialize")
    }

    fn sample_item() -> UiItemDto {
        UiItemDto {
            id: "toolbar.extract".to_string(),
            region: UiRegionDto::Toolbar,
            group_id: Some("file_actions".to_string()),
            label: "Extract".to_string(),
            icon: Some("EXPORT".to_string()),
            visible: true,
            sort_order: 101,
            display_mode: UiDisplayModeDto::IconAndText,
            action_type: UiActionTypeDto::Builtin,
            action_data: Some("payload".to_string()),
        }
    }

    // ========================================================================
    // Mirror fidelity. The exhaustive matches in these helpers carry no
    // wildcard arm, so a variant added upstream fails to compile here --
    // which is the point of a mirror type.
    // ========================================================================

    fn every_source_region() -> Vec<UiRegion> {
        let all = vec![
            UiRegion::Toolbar,
            UiRegion::ContextMenu,
            UiRegion::ToolsDialog,
            UiRegion::InfoPanel,
        ];
        for region in &all {
            match region {
                UiRegion::Toolbar
                | UiRegion::ContextMenu
                | UiRegion::ToolsDialog
                | UiRegion::InfoPanel => {}
            }
        }
        all
    }

    fn every_source_action_type() -> Vec<ActionType> {
        let all = vec![ActionType::Builtin, ActionType::Plugin, ActionType::Custom];
        for action_type in &all {
            match action_type {
                ActionType::Builtin | ActionType::Plugin | ActionType::Custom => {}
            }
        }
        all
    }

    fn every_source_display_mode() -> Vec<DisplayMode> {
        let all = vec![
            DisplayMode::IconAndText,
            DisplayMode::IconOnly,
            DisplayMode::TextOnly,
        ];
        for display_mode in &all {
            match display_mode {
                DisplayMode::IconAndText | DisplayMode::IconOnly | DisplayMode::TextOnly => {}
            }
        }
        all
    }

    #[test]
    fn every_region_variant_survives_the_round_trip_through_the_mirror() {
        let sources = every_source_region();
        let mirrored: Vec<UiRegionDto> = sources.iter().copied().map(UiRegionDto::from).collect();
        // Distinct in, distinct out: two source variants collapsing onto
        // one mirrored variant would still pass a per-value round trip.
        assert_eq!(
            mirrored
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            sources.len(),
            "each source region must map to its own mirrored variant"
        );
        for (source, mirrored) in sources.iter().copied().zip(mirrored) {
            assert_eq!(UiRegion::from(mirrored).as_str(), source.as_str());
            assert_eq!(round_trip(&mirrored), mirrored);
        }
    }

    #[test]
    fn every_action_type_variant_survives_the_round_trip_through_the_mirror() {
        let sources = every_source_action_type();
        let mirrored: Vec<UiActionTypeDto> =
            sources.iter().copied().map(UiActionTypeDto::from).collect();
        assert_eq!(
            mirrored
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            sources.len(),
            "each source action type must map to its own mirrored variant"
        );
        for (source, mirrored) in sources.iter().copied().zip(mirrored) {
            assert_eq!(ActionType::from(mirrored).as_str(), source.as_str());
            assert_eq!(round_trip(&mirrored), mirrored);
        }
    }

    #[test]
    fn every_display_mode_variant_survives_the_round_trip_through_the_mirror() {
        let sources = every_source_display_mode();
        let mirrored: Vec<UiDisplayModeDto> = sources
            .iter()
            .copied()
            .map(UiDisplayModeDto::from)
            .collect();
        assert_eq!(
            mirrored
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            sources.len(),
            "each source display mode must map to its own mirrored variant"
        );
        for (source, mirrored) in sources.iter().copied().zip(mirrored) {
            assert_eq!(DisplayMode::from(mirrored).as_str(), source.as_str());
            assert_eq!(round_trip(&mirrored), mirrored);
        }
    }

    /// The two defaults must agree, because the layout editor constructs
    /// plugin-contributed items with the *default* display mode and the
    /// stored row has to come out the same way.
    #[test]
    fn mirrored_defaults_match_the_stored_defaults() {
        assert_eq!(
            DisplayMode::from(UiDisplayModeDto::default()).as_str(),
            DisplayMode::default().as_str()
        );
        assert_eq!(
            ActionType::from(UiActionTypeDto::default()).as_str(),
            ActionType::default().as_str()
        );
    }

    #[test]
    fn an_item_survives_the_round_trip_through_the_stored_shape() {
        let dto = sample_item();
        let stored = UiItem::from(dto.clone());

        assert_eq!(stored.id, dto.id);
        assert_eq!(stored.region.as_str(), UiRegion::Toolbar.as_str());
        assert_eq!(stored.group_id, dto.group_id);
        assert_eq!(stored.label, dto.label);
        assert_eq!(stored.icon, dto.icon);
        assert_eq!(stored.visible, dto.visible);
        assert_eq!(stored.sort_order, dto.sort_order);
        assert_eq!(
            stored.display_mode.as_str(),
            DisplayMode::IconAndText.as_str()
        );
        assert_eq!(stored.action_type.as_str(), ActionType::Builtin.as_str());
        assert_eq!(stored.action_data, dto.action_data);

        assert_eq!(UiItemDto::from(stored), dto);
    }

    #[test]
    fn an_items_optional_fields_stay_absent_through_the_round_trip() {
        let dto = UiItemDto {
            id: "info.archive".to_string(),
            region: UiRegionDto::InfoPanel,
            group_id: None,
            label: "Archive Info".to_string(),
            icon: None,
            visible: false,
            sort_order: -3,
            display_mode: UiDisplayModeDto::TextOnly,
            action_type: UiActionTypeDto::Custom,
            action_data: None,
        };
        assert_eq!(UiItemDto::from(UiItem::from(dto.clone())), dto);
        assert_eq!(round_trip(&dto), dto);
    }

    #[test]
    fn item_json_uses_snake_case_field_and_variant_names() {
        let json = serde_json::to_value(sample_item()).expect("serialize");
        assert_eq!(json["id"], "toolbar.extract");
        assert_eq!(json["region"], "toolbar");
        assert_eq!(json["group_id"], "file_actions");
        assert_eq!(json["display_mode"], "icon_and_text");
        assert_eq!(json["action_type"], "builtin");
        assert_eq!(json["sort_order"], 101);
    }

    // ========================================================================
    // Display options.
    // ========================================================================

    #[test]
    fn display_options_round_trip_through_json() {
        let options = UiDisplayOptionsDto {
            default_view_mode: UiViewModeDto::Grid,
            tree_panel_visible: false,
            tree_panel_width: 321.5,
            properties_panel_visible: false,
            properties_panel_width: 456.25,
            show_button_labels: true,
        };
        assert_eq!(round_trip(&options), options);
    }

    #[test]
    fn view_mode_parses_only_grid_as_grid() {
        assert_eq!(UiViewModeDto::from_stored("grid"), UiViewModeDto::Grid);
        assert_eq!(UiViewModeDto::from_stored("list"), UiViewModeDto::List);
        assert_eq!(UiViewModeDto::from_stored(""), UiViewModeDto::List);
        assert_eq!(UiViewModeDto::from_stored("Grid"), UiViewModeDto::List);
        assert_eq!(UiViewModeDto::Grid.as_stored(), "grid");
        assert_eq!(UiViewModeDto::List.as_stored(), "list");
        assert_eq!(
            UiViewModeDto::from_stored(UiViewModeDto::default().as_stored()),
            UiViewModeDto::default()
        );
    }

    #[test]
    fn stored_bool_treats_only_the_true_token_as_true() {
        assert!(stored_bool(Some("true".to_string()), false));
        assert!(!stored_bool(Some("false".to_string()), true));
        assert!(!stored_bool(Some("TRUE".to_string()), true));
        assert!(stored_bool(None, true));
        assert!(!stored_bool(None, false));
    }

    #[test]
    fn stored_width_falls_back_for_anything_unusable() {
        assert_eq!(stored_width(Some("240".to_string()), 200.0), 240.0);
        assert_eq!(stored_width(Some("240.5".to_string()), 200.0), 240.5);
        assert_eq!(stored_width(None, 200.0), 200.0);
        assert_eq!(stored_width(Some("wide".to_string()), 200.0), 200.0);
        assert_eq!(stored_width(Some("NaN".to_string()), 200.0), 200.0);
        assert_eq!(stored_width(Some("-1".to_string()), 200.0), 200.0);
        assert_eq!(stored_width(Some("1e30".to_string()), 200.0), 200.0);
    }

    /// A default-constructed value is what a read of an empty database
    /// produces, so the two must agree field for field or a fresh profile
    /// and an unconfigured one would disagree.
    #[test]
    fn default_display_options_match_what_an_unset_database_reads_as() {
        let defaults = UiDisplayOptionsDto::default();
        assert_eq!(
            UiViewModeDto::from_stored(""),
            defaults.default_view_mode,
            "an unset view mode reads as the default"
        );
        assert_eq!(stored_bool(None, true), defaults.tree_panel_visible);
        assert_eq!(stored_width(None, 200.0), defaults.tree_panel_width);
        assert_eq!(stored_bool(None, true), defaults.properties_panel_visible);
        assert_eq!(stored_width(None, 280.0), defaults.properties_panel_width);
        assert_eq!(stored_bool(None, false), defaults.show_button_labels);
    }

    #[test]
    fn default_display_options_are_accepted_by_the_write_path() {
        check_display_options(&UiDisplayOptionsDto::default()).expect("defaults must be storable");
    }

    fn with_tree_panel_width(width: f32) -> UiDisplayOptionsDto {
        UiDisplayOptionsDto {
            tree_panel_width: width,
            ..UiDisplayOptionsDto::default()
        }
    }

    #[test]
    fn a_non_finite_panel_width_is_refused() {
        let error = check_display_options(&with_tree_panel_width(f32::NAN))
            .expect_err("NaN is not a width");
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("tree_panel_width"));

        let error = check_display_options(&UiDisplayOptionsDto {
            properties_panel_width: f32::INFINITY,
            ..UiDisplayOptionsDto::default()
        })
        .expect_err("infinity is not a width");
        assert_eq!(error.field.as_deref(), Some("properties_panel_width"));
    }

    #[test]
    fn an_absurd_or_negative_panel_width_is_refused() {
        assert!(check_display_options(&with_tree_panel_width(-1.0)).is_err());
        assert!(
            check_display_options(&with_tree_panel_width(MAX_UI_PANEL_WIDTH_PX + 1.0)).is_err()
        );
    }

    // ========================================================================
    // Item write-path validation.
    // ========================================================================

    #[test]
    fn a_matching_batch_converts_to_the_stored_shape() {
        let items = vec![sample_item()];
        let stored = items_to_core(UiRegionDto::Toolbar, items).expect("a matching batch is valid");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, "toolbar.extract");
    }

    #[test]
    fn an_empty_batch_is_valid() {
        assert!(items_to_core(UiRegionDto::Toolbar, Vec::new())
            .expect("an empty batch writes nothing")
            .is_empty());
    }

    #[test]
    fn an_item_from_another_region_is_refused() {
        let error = items_to_core(UiRegionDto::InfoPanel, vec![sample_item()])
            .expect_err("a toolbar item must not be saved into the info panel");
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("items[0].region"));
        assert_eq!(error.recoverability, Recoverability::UserAction);
    }

    #[test]
    fn an_empty_id_is_refused() {
        let mut item = sample_item();
        item.id = String::new();
        let error =
            items_to_core(UiRegionDto::Toolbar, vec![item]).expect_err("an empty id is not a key");
        assert_eq!(error.field.as_deref(), Some("items[0].id"));
    }

    #[test]
    fn a_duplicate_id_in_one_batch_is_refused() {
        let error = items_to_core(UiRegionDto::Toolbar, vec![sample_item(), sample_item()])
            .expect_err("the second upsert would silently win");
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("items[1].id"));
    }

    #[test]
    fn an_over_long_text_field_is_refused_naming_the_field() {
        type Mutate = fn(&mut UiItemDto, String);
        let cases: [(&str, Mutate); 5] = [
            ("items[0].id", |item, text| item.id = text),
            ("items[0].label", |item, text| item.label = text),
            ("items[0].group_id", |item, text| item.group_id = Some(text)),
            ("items[0].icon", |item, text| item.icon = Some(text)),
            ("items[0].action_data", |item, text| {
                item.action_data = Some(text)
            }),
        ];

        for (field, mutate) in cases {
            let mut item = sample_item();
            mutate(&mut item, "x".repeat(MAX_UI_ITEM_TEXT_BYTES + 1));
            let error = items_to_core(UiRegionDto::Toolbar, vec![item])
                .expect_err("text over the bound must be refused");
            assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
            assert_eq!(
                error.field.as_deref(),
                Some(field),
                "the refusal must name the field that was too long"
            );
        }
    }

    #[test]
    fn text_exactly_at_the_bound_is_accepted() {
        let mut item = sample_item();
        item.label = "x".repeat(MAX_UI_ITEM_TEXT_BYTES);
        items_to_core(UiRegionDto::Toolbar, vec![item]).expect("the bound itself is inclusive");
    }

    #[test]
    fn a_batch_over_the_item_bound_is_refused_before_any_item_is_examined() {
        let items: Vec<UiItemDto> = (0..=MAX_UI_ITEMS_PER_REGION)
            .map(|index| {
                let mut item = sample_item();
                item.id = format!("toolbar.item{index}");
                item
            })
            .collect();
        let error = items_to_core(UiRegionDto::Toolbar, items)
            .expect_err("an oversized batch must be refused");
        assert_eq!(error.field.as_deref(), Some("items"));
    }

    #[test]
    fn a_batch_exactly_at_the_item_bound_is_accepted() {
        let items: Vec<UiItemDto> = (0..MAX_UI_ITEMS_PER_REGION)
            .map(|index| {
                let mut item = sample_item();
                item.id = format!("toolbar.item{index}");
                item
            })
            .collect();
        assert_eq!(
            items_to_core(UiRegionDto::Toolbar, items)
                .expect("the bound itself is inclusive")
                .len(),
            MAX_UI_ITEMS_PER_REGION
        );
    }
}
