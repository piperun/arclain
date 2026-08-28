//! Render-emit tests for the facade-backed plugin document renderer.
//!
//! Same shape as `render_emit_test.rs`: a headless `egui_kittest`
//! harness renders real widgets, the test clicks one, and asserts on the
//! [`DocumentEvent`]s the render returned. These pin the half of the
//! cutover that unit tests on the session registry cannot reach -- that a
//! *press* produces the right event -- and in particular that button
//! navigation never enters the plugin action channel.

use arclain_app::ids::PluginSessionId;
use arclain_app::plugins::{
    PluginActionDto, PluginButtonActionDto, PluginExtensionPointDto, PluginUiDocument,
    PluginUiNodeDto, PluginUiNodeKind, SidebarWidth, SizeHint, SpacingStep, TextRole,
};
use arclain_ui::features::plugins::application::PluginNavigation;
use arclain_ui::features::plugins::presentation::rendering::{
    carousel_height_for_hint, image_height_for_hint, list_height_for_hint, render_document,
    sidebar_width_for_step, text_style_for_role, DocumentContext, DocumentEvent, DocumentExtent,
    RoleStyle,
};
use arclain_ui::shared::theme::AppTheme;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;

fn node(id: &str, kind: PluginUiNodeKind) -> PluginUiNodeDto {
    PluginUiNodeDto {
        id: id.to_string(),
        kind,
        visible: true,
        enabled: true,
    }
}

fn document(children: Vec<PluginUiNodeDto>) -> PluginUiDocument {
    PluginUiDocument {
        session_id: PluginSessionId::from_raw(1),
        plugin_id: "ui-demo".to_string(),
        region_id: "panel".to_string(),
        extension_point: PluginExtensionPointDto::Panel,
        revision: 1,
        root: node("#root", PluginUiNodeKind::Single { children }),
    }
}

fn button(id: &str, label: &str, action: Option<PluginButtonActionDto>) -> PluginUiNodeDto {
    node(
        id,
        PluginUiNodeKind::Button {
            label: label.to_string(),
            action,
        },
    )
}

struct Stage {
    document: PluginUiDocument,
    theme: AppTheme,
    extent: DocumentExtent,
    events: Vec<DocumentEvent>,
    /// Vertical room the document consumed, for the extent test.
    consumed: f32,
}

impl Stage {
    fn new(document: PluginUiDocument) -> Self {
        Self {
            document,
            theme: AppTheme::new(false),
            extent: DocumentExtent::Full,
            events: Vec::new(),
            consumed: 0.0,
        }
    }
}

fn harness(document: PluginUiDocument) -> Harness<'static, Stage> {
    Harness::new_ui_state(
        |ui, stage: &mut Stage| {
            let ctx = DocumentContext {
                colors: &stage.theme.colors,
                shared_state: None,
                image_owner: None,
                extent: stage.extent,
            };
            let mut events = render_document(ui, &stage.document, ctx);
            // Latch every event across frames: the click lands on one
            // frame and the harness runs several.
            stage.events.append(&mut events);
        },
        Stage::new(document),
    )
}

#[test]
fn a_plain_button_press_dispatches_the_nodes_own_id_to_the_plugin() {
    let mut harness = harness(document(vec![button("go", "Go", None)]));
    harness.run();
    harness.get_by_label("Go").click();
    harness.run();

    assert_eq!(
        harness.state().events,
        vec![DocumentEvent::Interact {
            expected_session_id: PluginSessionId::from_raw(1),
            expected_revision: 1,
            node_id: "go".to_string(),
            action: PluginActionDto::Activate,
        }]
    );
}

/// A `Custom` button action names a different event id than the node --
/// the flat renderer honored this too, and plugins rely on it to give one
/// button a distinct handler id.
#[test]
fn a_custom_button_action_dispatches_the_custom_id_not_the_node_id() {
    let mut harness = harness(document(vec![button(
        "go",
        "Go",
        Some(PluginButtonActionDto::Custom {
            value: "do_thing".to_string(),
        }),
    )]));
    harness.run();
    harness.get_by_label("Go").click();
    harness.run();

    assert_eq!(
        harness.state().events,
        vec![DocumentEvent::Interact {
            expected_session_id: PluginSessionId::from_raw(1),
            expected_revision: 1,
            node_id: "do_thing".to_string(),
            action: PluginActionDto::Activate,
        }]
    );
}

/// The core of the prefix reconciliation: every declarative navigation
/// action becomes host navigation, at *every* extension point, and none
/// of them produce an `Interact` the plugin would receive as a literal
/// `"__dialog_open:"`-style event id.
#[test]
fn every_declarative_button_action_becomes_host_navigation_and_never_a_plugin_event() {
    let cases = [
        (
            PluginButtonActionDto::ShowDialog {
                id: "settings".to_string(),
            },
            PluginNavigation::OpenDialog {
                dialog_id: "settings".to_string(),
            },
        ),
        (
            PluginButtonActionDto::CloseDialog,
            PluginNavigation::CloseDialog,
        ),
        (
            PluginButtonActionDto::OpenPage {
                id: "detail".to_string(),
            },
            PluginNavigation::OpenPage {
                page_id: "detail".to_string(),
            },
        ),
        (
            PluginButtonActionDto::ClosePage,
            PluginNavigation::ClosePage,
        ),
    ];

    for (action, expected) in cases {
        let mut harness = harness(document(vec![button(
            "nav",
            "Navigate",
            Some(action.clone()),
        )]));
        harness.run();
        harness.get_by_label("Navigate").click();
        harness.run();

        assert_eq!(
            harness.state().events,
            vec![DocumentEvent::Navigate(expected)],
            "{action:?} must resolve to host navigation"
        );
        assert!(
            !harness
                .state()
                .events
                .iter()
                .any(|event| matches!(event, DocumentEvent::Interact { .. })),
            "{action:?} must never reach the plugin action channel"
        );
    }
}

/// The application layer rejects an action against a disabled node before
/// it reaches the guest, so drawing it as pressable would show the user a
/// control whose presses silently vanish. The flat element type could not
/// express this at all.
#[test]
fn a_disabled_node_is_drawn_but_cannot_be_pressed() {
    let mut disabled = button("go", "Go", None);
    disabled.enabled = false;
    let mut harness = harness(document(vec![disabled]));
    harness.run();
    harness.get_by_label("Go").click();
    harness.run();

    assert!(harness.state().events.is_empty());
}

#[test]
fn a_hidden_node_is_not_drawn_at_all() {
    let mut hidden = button("go", "Go", None);
    hidden.visible = false;
    let mut harness = harness(document(vec![
        hidden,
        node(
            "visible_label",
            PluginUiNodeKind::Label {
                text: "Still here".to_string(),
                role: TextRole::Body,
            },
        ),
    ]));
    harness.run();

    assert!(harness.query_by_label("Go").is_none());
    assert!(harness.query_by_label("Still here").is_some());
}

/// Groups are real tree nodes after normalization, so their children
/// render inside them rather than being reconstructed from marker pairs.
#[test]
fn group_children_render_inside_the_group() {
    let mut harness = harness(document(vec![node(
        "group",
        PluginUiNodeKind::Group {
            title: "Options".to_string(),
            description: Some("Pick one".to_string()),
            children: vec![button("inner", "Inner", None)],
        },
    )]));
    harness.run();
    harness.get_by_label("Inner").click();
    harness.run();

    assert_eq!(
        harness.state().events,
        vec![DocumentEvent::Interact {
            expected_session_id: PluginSessionId::from_raw(1),
            expected_revision: 1,
            node_id: "inner".to_string(),
            action: PluginActionDto::Activate,
        }]
    );
}

/// A toolbar button is plain data inside a `Toolbar` node rather than a
/// node of its own, so its id names nothing in the tree -- the
/// application layer dispatches an unknown node id normally.
#[test]
fn a_toolbar_button_press_dispatches_its_own_id() {
    let mut harness = harness(document(vec![node(
        "bar",
        PluginUiNodeKind::Toolbar {
            buttons: vec![arclain_app::plugins::PluginToolbarButtonDto {
                id: "refresh".to_string(),
                label: "Refresh".to_string(),
                icon: None,
                primary: true,
                spacer_before: false,
            }],
        },
    )]));
    harness.run();
    harness.get_by_label("Refresh").click();
    harness.run();

    assert_eq!(
        harness.state().events,
        vec![DocumentEvent::Interact {
            expected_session_id: PluginSessionId::from_raw(1),
            expected_revision: 1,
            node_id: "refresh".to_string(),
            action: PluginActionDto::Activate,
        }]
    );
}

/// Two same-labelled buttons under distinct node ids must both be
/// individually addressable. Before node-scoped egui ids, widget ids were
/// whatever ambient scope the call site happened to open plus the
/// plugin's own element id, so a document rendered twice (a panel and a
/// dialog for one plugin) collided.
#[test]
fn sibling_nodes_with_identical_labels_stay_individually_addressable() {
    let mut harness = harness(document(vec![
        button("first", "Same", None),
        button("second", "Same", None),
    ]));
    harness.run();
    let buttons = harness.get_all_by_label("Same").collect::<Vec<_>>();
    assert_eq!(buttons.len(), 2);
    buttons[1].click();
    harness.run();

    assert_eq!(
        harness.state().events,
        vec![DocumentEvent::Interact {
            expected_session_id: PluginSessionId::from_raw(1),
            expected_revision: 1,
            node_id: "second".to_string(),
            action: PluginActionDto::Activate,
        }]
    );
}

/// A plugin names a step; the host picks the pixels. The three steps are
/// asserted against each other rather than against absolute heights, so the
/// gap the document wrapper adds around any single node cancels out and what
/// is left is purely the host's scale: 8, 12 and 20.
#[test]
fn each_spacing_step_costs_the_height_the_host_assigns_it() {
    fn consumed(step: SpacingStep) -> f32 {
        let mut harness = Harness::new_ui_state(
            |ui, stage: &mut Stage| {
                let before = ui.cursor().min.y;
                let ctx = DocumentContext {
                    colors: &stage.theme.colors,
                    shared_state: None,
                    image_owner: None,
                    extent: stage.extent,
                };
                let _ = render_document(ui, &stage.document, ctx);
                stage.consumed = ui.cursor().min.y - before;
            },
            Stage::new(document(vec![node(
                "gap",
                PluginUiNodeKind::Space { step },
            )])),
        );
        harness.run();
        harness.state().consumed
    }

    let small = consumed(SpacingStep::Small);
    let medium = consumed(SpacingStep::Medium);
    let large = consumed(SpacingStep::Large);

    assert_eq!(medium - small, 4.0, "medium is 12 where small is 8");
    assert_eq!(large - small, 12.0, "large is 20 where small is 8");
}

/// A `Split` in a stacked host is bounded so it cannot swallow the rest of
/// the panel; in a host that owns its container it is not.
#[test]
fn a_split_is_height_bounded_only_when_the_host_asks_for_it() {
    fn split_document() -> PluginUiDocument {
        let mut doc = document(Vec::new());
        doc.root = node(
            "#root",
            PluginUiNodeKind::Split {
                sidebar: vec![node(
                    "s",
                    PluginUiNodeKind::Label {
                        text: "Sidebar".to_string(),
                        role: TextRole::Body,
                    },
                )],
                content: vec![node(
                    "c",
                    PluginUiNodeKind::Label {
                        text: "Content".to_string(),
                        role: TextRole::Body,
                    },
                )],
                width: Some(SidebarWidth::Narrow),
            },
        );
        doc
    }

    let mut bounded = Harness::new_ui_state(
        |ui, stage: &mut Stage| {
            let before = ui.cursor().min.y;
            let ctx = DocumentContext {
                colors: &stage.theme.colors,
                shared_state: None,
                image_owner: None,
                extent: stage.extent,
            };
            let _ = render_document(ui, &stage.document, ctx);
            // Record how much vertical room the document consumed.
            stage.consumed = ui.cursor().min.y - before;
        },
        Stage {
            extent: DocumentExtent::Bounded(120),
            ..Stage::new(split_document())
        },
    );
    bounded.run();
    let bounded_height = bounded.state().consumed;

    let mut full = Harness::new_ui_state(
        |ui, stage: &mut Stage| {
            let before = ui.cursor().min.y;
            let ctx = DocumentContext {
                colors: &stage.theme.colors,
                shared_state: None,
                image_owner: None,
                extent: stage.extent,
            };
            let _ = render_document(ui, &stage.document, ctx);
            stage.consumed = ui.cursor().min.y - before;
        },
        Stage {
            extent: DocumentExtent::Full,
            ..Stage::new(split_document())
        },
    );
    full.run();

    // Both still render the real two-pane layout -- bounding must not
    // flatten the split away the way the pre-cutover panel path did.
    assert!(bounded.query_by_label("Sidebar").is_some());
    assert!(bounded.query_by_label("Content").is_some());
    assert!(
        bounded_height <= 140.0,
        "a bounded split must respect its cap, consumed {bounded_height}"
    );
    assert!(
        full.state().consumed > bounded_height,
        "an unbounded split must be free to take more room than a bounded one"
    );
}

/// A plugin names a role; this pins the host's answer. Every role has to
/// land on its own row of the type scale, or two of them say different
/// things and render as the same thing -- which is the failure a plugin
/// author cannot see and cannot work around.
#[test]
fn every_text_role_renders_differently() {
    let styles: Vec<RoleStyle> = [
        TextRole::Title,
        TextRole::Subtitle,
        TextRole::Body,
        TextRole::Caption,
        TextRole::Emphasis,
    ]
    .into_iter()
    .map(text_style_for_role)
    .collect();

    for (index, style) in styles.iter().enumerate() {
        assert!(
            !styles[index + 1..].contains(style),
            "two roles render identically: {style:?}"
        );
    }
}

/// The scale above is only worth pinning if the renderer reads it. The
/// retired code decided "this is a heading" from the declared size and then
/// drew every heading through a widget that sized itself, so a title and a
/// subtitle came out identical on screen while any table would have called
/// them distinct. Measuring real consumed height is what separates "the
/// numbers differ" from "the numbers reach the user".
///
/// Only the four roles with distinct sizes are ordered here. Emphasis and
/// body share a size and differ by weight, which does not move the line
/// box; `every_text_role_renders_differently` is what covers that pair.
#[test]
fn a_larger_role_takes_more_room_on_screen_than_a_smaller_one() {
    fn consumed(role: TextRole) -> f32 {
        let mut harness = Harness::new_ui_state(
            |ui, stage: &mut Stage| {
                let before = ui.cursor().min.y;
                let ctx = DocumentContext {
                    colors: &stage.theme.colors,
                    shared_state: None,
                    image_owner: None,
                    extent: stage.extent,
                };
                let _ = render_document(ui, &stage.document, ctx);
                stage.consumed = ui.cursor().min.y - before;
            },
            Stage::new(document(vec![node(
                "text",
                PluginUiNodeKind::Label {
                    text: "Ay".to_string(),
                    role,
                },
            )])),
        );
        harness.run();
        harness.state().consumed
    }

    let title = consumed(TextRole::Title);
    let subtitle = consumed(TextRole::Subtitle);
    let body = consumed(TextRole::Body);
    let caption = consumed(TextRole::Caption);

    assert!(
        title > subtitle,
        "a title must outgrow a subtitle, got {title} and {subtitle}"
    );
    assert!(
        subtitle > body,
        "a subtitle must outgrow body text, got {subtitle} and {body}"
    );
    assert!(
        body > caption,
        "body text must outgrow a caption, got {body} and {caption}"
    );
}

/// One vocabulary, three scales. A plugin names `Tall` and the host answers
/// 400 for an image, 700 for a list and 700 for a carousel -- if these three
/// ever collapse onto one table, the reason the plugin names a step instead
/// of a number disappears with them.
///
/// The absent hint is pinned too, and it is not one answer either: an image
/// with no hint has nothing to say to the renderer below it, while a list
/// always occupies a height and so must resolve to one.
#[test]
fn a_step_means_a_different_height_per_kind() {
    assert_eq!(image_height_for_hint(Some(SizeHint::Tall)), Some(400.0));
    assert_eq!(list_height_for_hint(Some(SizeHint::Tall)), 700.0);
    assert_eq!(carousel_height_for_hint(Some(SizeHint::Tall)), 700.0);

    assert_eq!(image_height_for_hint(Some(SizeHint::Regular)), Some(200.0));
    assert_eq!(carousel_height_for_hint(Some(SizeHint::Regular)), 400.0);

    assert_eq!(image_height_for_hint(None), None);
    assert_eq!(list_height_for_hint(None), 300.0);
    assert_eq!(carousel_height_for_hint(None), 300.0);
}

/// The scale above is only worth pinning if the renderer reads it. A table
/// test compares the host's numbers to themselves and would pass just as
/// green against a renderer that ignored the hint entirely, which is exactly
/// the shape of bug this vocabulary exists to prevent.
///
/// The list container is the kind that can be measured without an asset: it
/// bounds a scroll area, so overflowing content makes the resolved number
/// the height on screen. An image and a carousel need a real texture before
/// they occupy anything, so `a_step_means_a_different_height_per_kind` is
/// what covers those two.
#[test]
fn a_taller_list_hint_bounds_the_list_higher_on_screen() {
    fn consumed(height: Option<SizeHint>) -> f32 {
        let rows: Vec<PluginUiNodeDto> = (0..60)
            .map(|index| {
                node(
                    &format!("row-{index}"),
                    PluginUiNodeKind::Label {
                        text: "Row".to_string(),
                        role: TextRole::Body,
                    },
                )
            })
            .collect();
        let mut harness = Harness::new_ui_state(
            |ui, stage: &mut Stage| {
                let before = ui.cursor().min.y;
                let ctx = DocumentContext {
                    colors: &stage.theme.colors,
                    shared_state: None,
                    image_owner: None,
                    extent: stage.extent,
                };
                let _ = render_document(ui, &stage.document, ctx);
                stage.consumed = ui.cursor().min.y - before;
            },
            Stage::new(document(vec![node(
                "list",
                PluginUiNodeKind::ListContainer {
                    children: rows,
                    height,
                    empty_message: None,
                },
            )])),
        );
        harness.run();
        harness.state().consumed
    }

    let compact = consumed(Some(SizeHint::Compact));
    let absent = consumed(None);
    let regular = consumed(Some(SizeHint::Regular));

    // 200 against 300: the gap is the host's scale, and the frame's own
    // margins cancel because both sides carry the same frame.
    assert_eq!(
        regular - compact,
        100.0,
        "regular bounds a list 100px higher than compact, got {compact} and {regular}"
    );
    assert_eq!(
        absent, regular,
        "a list with no hint takes the same room as a regular one"
    );
}

/// A plugin names how much of the pane its sidebar wants; this pins the
/// host's answer. The absent step is pinned alongside the three named ones
/// because it is the case a plugin reaches by saying nothing at all, and it
/// has to land on a real width rather than on nothing -- a split always
/// draws a sidebar.
#[test]
fn a_sidebar_step_is_a_width_the_host_owns() {
    assert_eq!(sidebar_width_for_step(Some(SidebarWidth::Narrow)), 200.0);
    assert_eq!(sidebar_width_for_step(Some(SidebarWidth::Medium)), 250.0);
    assert_eq!(sidebar_width_for_step(Some(SidebarWidth::Wide)), 300.0);
    assert_eq!(sidebar_width_for_step(None), 250.0);
}

/// Renders a split and reports where the content pane's own label starts.
/// The content pane begins where the sidebar ends, so this is the resolved
/// sidebar width arriving on screen.
fn split_content_left(width: Option<SidebarWidth>, sidebar: Vec<PluginUiNodeDto>) -> f32 {
    let mut document = document(Vec::new());
    document.root = node(
        "#root",
        PluginUiNodeKind::Split {
            sidebar,
            content: vec![node(
                "c",
                PluginUiNodeKind::Label {
                    text: "Content".to_string(),
                    role: TextRole::Body,
                },
            )],
            width,
        },
    );
    let mut stage = harness(document);
    stage.run();
    stage.get_by_label("Content").rect().left()
}

/// One short label: the sparsest sidebar a plugin can write, and the one
/// that stopped holding its width.
fn sparse_sidebar() -> Vec<PluginUiNodeDto> {
    vec![node(
        "s",
        PluginUiNodeKind::Label {
            text: "Sidebar".to_string(),
            role: TextRole::Body,
        },
    )]
}

/// The scale above is only worth pinning if the renderer reads it. A table
/// test compares the host's numbers to themselves and would pass just as
/// green against a renderer that pinned one width and ignored the step,
/// which is the shape of bug this vocabulary exists to prevent.
#[test]
fn a_wider_sidebar_step_pushes_the_content_pane_further_right() {
    let narrow = split_content_left(Some(SidebarWidth::Narrow), sparse_sidebar());
    let medium = split_content_left(Some(SidebarWidth::Medium), sparse_sidebar());
    let wide = split_content_left(Some(SidebarWidth::Wide), sparse_sidebar());
    let absent = split_content_left(None, sparse_sidebar());

    // 200, 250 and 300: the gaps are the host's scale, and every other
    // margin cancels because all four documents are otherwise identical.
    assert_eq!(
        medium - narrow,
        50.0,
        "medium starts the content pane 50px right of narrow, got {narrow} and {medium}"
    );
    assert_eq!(
        wide - narrow,
        100.0,
        "wide starts the content pane 100px right of narrow, got {narrow} and {wide}"
    );
    assert_eq!(
        absent, medium,
        "a split with no step gets the width a medium one gets"
    );
}

/// What a plugin puts in its sidebar must not decide how wide the sidebar
/// is -- the step decides, and the host answers the step. A `SidePanel`
/// remembers the rect its contents occupied and reuses it as its width from
/// the next frame on, so a sidebar holding one short label used to collapse
/// to a 96px floor a frame after being drawn at the width it asked for. A
/// plugin author would see a `Wide` sidebar come out narrower than a
/// `Narrow` one whose content happened to be wide, with nothing in their own
/// code to explain it.
///
/// A rule spans whatever room it is given, so the filled sidebar is the case
/// that never collapsed; the sparse one has to land in exactly the same
/// place.
#[test]
fn a_sparse_sidebar_keeps_the_width_the_host_assigned() {
    let filled = split_content_left(
        Some(SidebarWidth::Wide),
        vec![node("rule", PluginUiNodeKind::Separator)],
    );
    let sparse = split_content_left(Some(SidebarWidth::Wide), sparse_sidebar());

    assert_eq!(
        sparse, filled,
        "a sidebar holding one short label must be as wide as one holding a full-width rule, \
         got {sparse} and {filled}"
    );
}

/// A list item reserves width for whatever trails its title. In a pane
/// narrower than that reservation the subtraction went negative, and the
/// title was handed a width no label can be drawn in.
///
/// egui does not refuse a negative maximum width the way it refuses a
/// negative height, so this never announced itself -- the row simply
/// stopped showing its title.
#[test]
fn a_list_item_keeps_its_title_in_a_pane_narrower_than_its_trailing_space() {
    let mut harness = Harness::builder()
        .with_size(eframe::egui::vec2(48.0, 200.0))
        .build_ui_state(
            |ui, stage: &mut Stage| {
                let ctx = DocumentContext {
                    colors: &stage.theme.colors,
                    shared_state: None,
                    image_owner: None,
                    extent: stage.extent,
                };
                let mut events = render_document(ui, &stage.document, ctx);
                stage.events.append(&mut events);
            },
            Stage::new(document(vec![node(
                "row",
                PluginUiNodeKind::ListItem {
                    title: "Placeholder".to_string(),
                    subtitle: None,
                    badge: None,
                    image_key: None,
                    image_url: None,
                    selected: false,
                    warning_icon: None,
                },
            )])),
        );

    harness.run();

    assert!(
        harness
            .query_all_by_label_contains("Placeholder")
            .next()
            .is_some(),
        "the title must still be rendered when the row is narrower than its reservation"
    );
}
