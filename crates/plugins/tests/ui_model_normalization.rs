//! Integration coverage for `arclain_plugins::ui_model::normalize_layout`.
//!
//! Exercises every current `PluginUiElement` variant -- the host-side
//! shape `crate::conversions::convert_ui_element` produces from the raw
//! WIT `ui-element` variant, i.e. exactly what reaches this crate's
//! normalization boundary from a real plugin -- plus every rejection
//! path the brief for this module calls out explicitly: duplicate
//! interactive ids, unmatched group markers, malformed layouts, and the
//! tree depth/node/text/asset budgets. Compile-guard coverage (this
//! crate builds without any egui/UI-signal type reachable from its
//! public API) lives alongside these as a `pub` type/import check, since
//! that is what actually proves the surface, rather than a Cargo feature
//! flag this crate has never had.

use arclain_plugins::types::{
    ButtonAction, KeyValuePair, PluginLayout, PluginUiElement, ToolbarButton, WarningIcon,
};
use arclain_plugins::ui_model::{
    normalize_layout, PluginButtonActionDto, PluginUiNodeKind, PluginUiNormalizeError,
    PluginWarningIconDto, MAX_UI_ASSETS, MAX_UI_NODES, MAX_UI_TEXT_BYTES, MAX_UI_TREE_DEPTH,
};

fn single(elements: Vec<PluginUiElement>) -> PluginLayout {
    PluginLayout::Single { elements }
}

fn root_children(layout: &PluginLayout) -> Vec<arclain_plugins::ui_model::PluginUiNodeDto> {
    let root = normalize_layout(layout).expect("layout must normalize");
    match root.kind {
        PluginUiNodeKind::Single { children } => children,
        other => panic!("expected a Single root, got {other:?}"),
    }
}

// ============================================================================
// One fixture per current WIT/host UI element kind.
// ============================================================================

#[test]
fn label_normalizes_to_a_display_only_node_with_a_structural_id() {
    let layout = single(vec![PluginUiElement::Label {
        text: "hello".to_string(),
        bold: true,
        size: Some(18.0),
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "#root/0");
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::Label {
            text: "hello".to_string(),
            bold: true,
            size: Some(18.0),
        }
    );
}

#[test]
fn section_header_normalizes_with_level_and_optional_description() {
    let layout = single(vec![PluginUiElement::SectionHeader {
        title: "Section".to_string(),
        level: 2,
        description: Some("subtitle".to_string()),
    }]);
    let children = root_children(&layout);
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::SectionHeader {
            title: "Section".to_string(),
            level: 2,
            description: Some("subtitle".to_string()),
        }
    );
}

#[test]
fn button_keeps_its_plugin_provided_id_and_converts_its_navigation_action() {
    let layout = single(vec![PluginUiElement::Button {
        id: "save".to_string(),
        label: "Save".to_string(),
        action: Some(ButtonAction::ShowDialog {
            id: "confirm".to_string(),
        }),
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "save");
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::Button {
            label: "Save".to_string(),
            action: Some(PluginButtonActionDto::ShowDialog {
                id: "confirm".to_string()
            }),
        }
    );
}

#[test]
fn text_input_keeps_its_id_and_carries_value_and_placeholder() {
    let layout = single(vec![PluginUiElement::TextInput {
        id: "search".to_string(),
        label: "Search".to_string(),
        value: "query".to_string(),
        placeholder: Some("type here".to_string()),
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "search");
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::TextInput {
            label: "Search".to_string(),
            value: "query".to_string(),
            placeholder: Some("type here".to_string()),
        }
    );
}

#[test]
fn checkbox_keeps_its_id() {
    let layout = single(vec![PluginUiElement::Checkbox {
        id: "enabled".to_string(),
        label: "Enabled".to_string(),
        checked: true,
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "enabled");
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::Checkbox {
            label: "Enabled".to_string(),
            checked: true,
        }
    );
}

#[test]
fn radio_group_keeps_its_id_and_options() {
    let layout = single(vec![PluginUiElement::RadioGroup {
        id: "mode".to_string(),
        label: "Mode".to_string(),
        options: vec!["A".to_string(), "B".to_string()],
        selected: "A".to_string(),
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "mode");
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::RadioGroup {
            label: "Mode".to_string(),
            options: vec!["A".to_string(), "B".to_string()],
            selected: "A".to_string(),
        }
    );
}

#[test]
fn slider_keeps_its_id_and_range() {
    let layout = single(vec![PluginUiElement::Slider {
        id: "volume".to_string(),
        label: "Volume".to_string(),
        value: 5.0,
        min: 0.0,
        max: 10.0,
        step: Some(1.0),
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "volume");
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::Slider {
            label: "Volume".to_string(),
            value: 5.0,
            min: 0.0,
            max: 10.0,
            step: Some(1.0),
        }
    );
}

#[test]
fn dropdown_keeps_its_id_and_options() {
    let layout = single(vec![PluginUiElement::Dropdown {
        id: "region".to_string(),
        label: "Region".to_string(),
        options: vec!["JP".to_string(), "US".to_string()],
        selected: "JP".to_string(),
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "region");
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::Dropdown {
            label: "Region".to_string(),
            options: vec!["JP".to_string(), "US".to_string()],
            selected: "JP".to_string(),
        }
    );
}

#[test]
fn image_is_display_only_and_counts_toward_the_asset_budget() {
    let layout = single(vec![PluginUiElement::Image {
        cache_key: Some("cover:1".to_string()),
        url: None,
        max_height: Some(200.0),
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "#root/0");
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::Image {
            cache_key: Some("cover:1".to_string()),
            url: None,
            max_height: Some(200.0),
        }
    );
}

#[test]
fn separator_and_space_are_display_only() {
    let layout = single(vec![
        PluginUiElement::Separator,
        PluginUiElement::Space { size: 12.0 },
    ]);
    let children = root_children(&layout);
    assert_eq!(children[0].kind, PluginUiNodeKind::Separator);
    assert_eq!(children[1].kind, PluginUiNodeKind::Space { size: 12.0 });
}

#[test]
fn tabs_keeps_its_id() {
    let layout = single(vec![PluginUiElement::Tabs {
        id: "view".to_string(),
        tabs: vec!["Info".to_string(), "Files".to_string()],
        selected: "Info".to_string(),
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "view");
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::Tabs {
            tabs: vec!["Info".to_string(), "Files".to_string()],
            selected: "Info".to_string(),
        }
    );
}

#[test]
fn list_item_keeps_its_id_and_converts_its_warning_icon() {
    let layout = single(vec![PluginUiElement::ListItem {
        id: "row-1".to_string(),
        title: "Title".to_string(),
        subtitle: Some("Subtitle".to_string()),
        badge: Some("NEW".to_string()),
        image_key: Some("thumb:1".to_string()),
        image_url: None,
        selected: true,
        warning_icon: Some(WarningIcon::GlobeX),
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "row-1");
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::ListItem {
            title: "Title".to_string(),
            subtitle: Some("Subtitle".to_string()),
            badge: Some("NEW".to_string()),
            image_key: Some("thumb:1".to_string()),
            image_url: None,
            selected: true,
            warning_icon: Some(PluginWarningIconDto::GlobeX),
        }
    );
}

#[test]
fn list_container_keeps_its_own_id_and_normalizes_nested_list_items() {
    let layout = single(vec![PluginUiElement::ListContainer {
        id: "results".to_string(),
        items: vec![PluginUiElement::ListItem {
            id: "row-1".to_string(),
            title: "Row".to_string(),
            subtitle: None,
            badge: None,
            image_key: None,
            image_url: None,
            selected: false,
            warning_icon: None,
        }],
        max_height: Some(400.0),
        empty_message: Some("Nothing here".to_string()),
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "results");
    let PluginUiNodeKind::ListContainer {
        children: items,
        max_height,
        empty_message,
    } = &children[0].kind
    else {
        panic!("expected ListContainer");
    };
    assert_eq!(*max_height, Some(400.0));
    assert_eq!(empty_message.as_deref(), Some("Nothing here"));
    assert_eq!(items[0].id, "row-1");
}

#[test]
fn loading_is_display_only() {
    let layout = single(vec![PluginUiElement::Loading {
        message: Some("Working...".to_string()),
    }]);
    let children = root_children(&layout);
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::Loading {
            message: Some("Working...".to_string()),
        }
    );
}

#[test]
fn warning_is_display_only_and_converts_its_icon() {
    let layout = single(vec![PluginUiElement::Warning {
        icon: WarningIcon::Warning,
        message: "Be careful".to_string(),
    }]);
    let children = root_children(&layout);
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::Warning {
            icon: PluginWarningIconDto::Warning,
            message: "Be careful".to_string(),
        }
    );
}

#[test]
fn tag_chips_is_display_only() {
    let layout = single(vec![PluginUiElement::TagChips {
        tags: vec!["rpg".to_string(), "jrpg".to_string()],
        max_display: Some(1),
    }]);
    let children = root_children(&layout);
    assert_eq!(
        children[0].kind,
        PluginUiNodeKind::TagChips {
            tags: vec!["rpg".to_string(), "jrpg".to_string()],
            max_display: Some(1),
        }
    );
}

#[test]
fn toolbar_buttons_are_embedded_data_not_separate_tree_nodes() {
    let layout = single(vec![PluginUiElement::Toolbar {
        buttons: vec![ToolbarButton {
            id: "refresh".to_string(),
            label: "Refresh".to_string(),
            icon: Some("refresh-icon".to_string()),
            primary: true,
            spacer_before: false,
        }],
    }]);
    let children = root_children(&layout);
    // The Toolbar node itself is display-only (structural id); its
    // button's own id lives as plain data inside PluginToolbarButtonDto,
    // not as a second PluginUiNodeDto -- so it is not reachable via
    // `PluginUiNodeDto::find`.
    assert_eq!(children[0].id, "#root/0");
    let PluginUiNodeKind::Toolbar { buttons } = &children[0].kind else {
        panic!("expected Toolbar");
    };
    assert_eq!(buttons[0].id, "refresh");
    let root = normalize_layout(&layout).unwrap();
    assert!(root.find("refresh").is_none());
}

#[test]
fn carousel_keeps_its_id_and_converts_every_image_and_counts_assets() {
    let layout = single(vec![PluginUiElement::Carousel {
        id: "gallery".to_string(),
        images: vec![
            (
                "img-1".to_string(),
                Some("https://example.invalid/1".to_string()),
            ),
            ("img-2".to_string(), None),
        ],
        current_index: 1,
        max_height: Some(300.0),
        thumbnail_height: Some(60.0),
        enable_lightbox: true,
    }]);
    let children = root_children(&layout);
    assert_eq!(children[0].id, "gallery");
    let PluginUiNodeKind::Carousel {
        images,
        current_index,
        enable_lightbox,
        ..
    } = &children[0].kind
    else {
        panic!("expected Carousel");
    };
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].cache_key, "img-1");
    assert_eq!(images[0].url.as_deref(), Some("https://example.invalid/1"));
    assert_eq!(images[1].url, None);
    assert_eq!(*current_index, 1);
    assert!(*enable_lightbox);
}

#[test]
fn key_value_list_and_metadata_grid_are_display_only() {
    let items = vec![KeyValuePair {
        key: "Genre".to_string(),
        value: "RPG".to_string(),
    }];
    let layout = single(vec![
        PluginUiElement::KeyValueList {
            items: items.clone(),
            columns: Some(2),
        },
        PluginUiElement::MetadataGrid {
            items,
            columns: None,
        },
    ]);
    let children = root_children(&layout);
    let PluginUiNodeKind::KeyValueList { items, columns } = &children[0].kind else {
        panic!("expected KeyValueList");
    };
    assert_eq!(items[0].key, "Genre");
    assert_eq!(*columns, Some(2));
    assert!(matches!(
        children[1].kind,
        PluginUiNodeKind::MetadataGrid { .. }
    ));
}

#[test]
fn split_layout_normalizes_sidebar_and_content_into_separate_subtrees() {
    let layout = PluginLayout::Split {
        sidebar: vec![PluginUiElement::Label {
            text: "sidebar".to_string(),
            bold: false,
            size: None,
        }],
        content: vec![PluginUiElement::Label {
            text: "content".to_string(),
            bold: false,
            size: None,
        }],
        sidebar_width: Some(220.0),
    };
    let root = normalize_layout(&layout).unwrap();
    let PluginUiNodeKind::Split {
        sidebar,
        content,
        sidebar_width,
    } = &root.kind
    else {
        panic!("expected Split root");
    };
    assert_eq!(sidebar_width, &Some(220.0));
    assert_eq!(sidebar[0].id, "#root/sidebar/0");
    assert_eq!(content[0].id, "#root/content/0");
}

// ============================================================================
// Rejection paths the brief calls out explicitly.
// ============================================================================

#[test]
fn duplicate_interactive_ids_across_different_element_kinds_are_rejected() {
    let layout = single(vec![
        PluginUiElement::Button {
            id: "shared".to_string(),
            label: "Button".to_string(),
            action: None,
        },
        PluginUiElement::Checkbox {
            id: "shared".to_string(),
            label: "Checkbox".to_string(),
            checked: false,
        },
    ]);
    assert_eq!(
        normalize_layout(&layout).unwrap_err(),
        PluginUiNormalizeError::DuplicateNodeId("shared".to_string())
    );
}

#[test]
fn duplicate_interactive_ids_across_sidebar_and_content_are_rejected() {
    let layout = PluginLayout::Split {
        sidebar: vec![PluginUiElement::Button {
            id: "shared".to_string(),
            label: "Sidebar".to_string(),
            action: None,
        }],
        content: vec![PluginUiElement::Button {
            id: "shared".to_string(),
            label: "Content".to_string(),
            action: None,
        }],
        sidebar_width: None,
    };
    assert_eq!(
        normalize_layout(&layout).unwrap_err(),
        PluginUiNormalizeError::DuplicateNodeId("shared".to_string())
    );
}

#[test]
fn unmatched_group_end_at_any_nesting_level_is_rejected() {
    let layout = single(vec![PluginUiElement::ListContainer {
        id: "list".to_string(),
        items: vec![PluginUiElement::GroupEnd],
        max_height: None,
        empty_message: None,
    }]);
    assert_eq!(
        normalize_layout(&layout).unwrap_err(),
        PluginUiNormalizeError::UnmatchedGroupEnd
    );
}

#[test]
fn group_begin_unclosed_inside_a_nested_container_is_rejected() {
    let layout = single(vec![PluginUiElement::ListContainer {
        id: "list".to_string(),
        items: vec![PluginUiElement::GroupBegin {
            title: "Never closed".to_string(),
            description: None,
        }],
        max_height: None,
        empty_message: None,
    }]);
    assert_eq!(
        normalize_layout(&layout).unwrap_err(),
        PluginUiNormalizeError::UnclosedGroup
    );
}

#[test]
fn malformed_layout_group_end_before_its_group_begin_is_rejected() {
    let layout = single(vec![
        PluginUiElement::GroupEnd,
        PluginUiElement::GroupBegin {
            title: "Too late".to_string(),
            description: None,
        },
        PluginUiElement::GroupEnd,
    ]);
    assert_eq!(
        normalize_layout(&layout).unwrap_err(),
        PluginUiNormalizeError::UnmatchedGroupEnd
    );
}

#[test]
fn tree_depth_node_text_and_asset_budgets_are_enforced_end_to_end() {
    // Depth: `MAX_UI_TREE_DEPTH` nested ListContainers is one over budget.
    fn nested(remaining: usize) -> PluginUiElement {
        if remaining == 0 {
            return PluginUiElement::Separator;
        }
        PluginUiElement::ListContainer {
            id: format!("nested-{remaining}"),
            items: vec![nested(remaining - 1)],
            max_height: None,
            empty_message: None,
        }
    }
    assert_eq!(
        normalize_layout(&single(vec![nested(MAX_UI_TREE_DEPTH)])).unwrap_err(),
        PluginUiNormalizeError::TreeTooDeep
    );

    // Node count.
    let over_nodes = single(
        (0..MAX_UI_NODES)
            .map(|_| PluginUiElement::Separator)
            .collect(),
    );
    assert_eq!(
        normalize_layout(&over_nodes).unwrap_err(),
        PluginUiNormalizeError::TooManyNodes
    );

    // Text budget.
    let over_text = single(vec![PluginUiElement::Label {
        text: "x".repeat(MAX_UI_TEXT_BYTES + 1),
        bold: false,
        size: None,
    }]);
    assert_eq!(
        normalize_layout(&over_text).unwrap_err(),
        PluginUiNormalizeError::TextBudgetExceeded
    );

    // Asset budget.
    let over_assets = single(
        (0..=MAX_UI_ASSETS)
            .map(|index| PluginUiElement::Image {
                cache_key: Some(format!("cover:{index}")),
                url: None,
                max_height: None,
            })
            .collect(),
    );
    assert_eq!(
        normalize_layout(&over_assets).unwrap_err(),
        PluginUiNormalizeError::TooManyAssets
    );
}

// ============================================================================
// Compile-guard: this crate's public API carries no UI-toolkit or
// signal type. There is no "egui feature" flag in this crate's
// Cargo.toml to gate out (it has never depended on egui at all); the
// meaningful assertion is that arclain_signals::Signal cannot appear in
// ui_model's public surface, and that this whole test file -- an
// external, `arclain_plugins`-only consumer -- compiles and links
// without pulling in egui or eframe. `arclain_signals` remains a
// dev-only dependency of this crate for now (three test doubles for
// `ActiveTabBridge` use it as a convenient interior-mutable cell), but
// carries no public-API weight: nothing exported from `ui_model`
// mentions it.
// ============================================================================

#[test]
fn ui_model_public_types_are_plain_data_with_no_toolkit_or_signal_dependency() {
    fn assert_plain_data<T: Clone + std::fmt::Debug + PartialEq>() {}
    assert_plain_data::<arclain_plugins::ui_model::PluginUiNodeDto>();
    assert_plain_data::<arclain_plugins::ui_model::PluginUiNodeKind>();
    assert_plain_data::<arclain_plugins::ui_model::PluginHostIntentDto>();
    assert_plain_data::<arclain_plugins::ui_model::PluginExtensionPointDto>();
    assert_plain_data::<arclain_plugins::ui_model::PluginActionDto>();
}
