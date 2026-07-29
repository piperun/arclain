//! Renders a facade-supplied [`PluginUiDocument`] -- the renderer half of
//! the cutover described in
//! `crate::features::plugins::application::facade_sessions`.
//!
//! # How this differs from the flat `PluginUiElement` renderer
//!
//! `super::renderer` walks a *flat* `Vec<PluginUiElement>` and
//! reconstructs nesting from `GroupBegin`/`GroupEnd` marker pairs at
//! render time. A document is already a tree
//! (`arclain_plugins::ui_model::normalize_layout` resolved the markers,
//! and rejects an unbalanced pair outright instead of silently absorbing
//! or skipping it), so this walks children directly. Three further
//! differences are deliberate, not incidental:
//!
//! - **Every node has a stable id, and every node gets its own egui id
//!   scope.** The old renderer's widget ids were whatever ambient
//!   `egui::Id` scope the call site happened to have opened, plus the
//!   plugin's own element id -- or, for several widget kinds, a bare
//!   literal shared by every instance of that kind (`Grid::new(
//!   "key_value_list")` is the same id for every key-value list in the
//!   window). Two plugins each rendering a key-value list, or one plugin
//!   rendering the same list in a panel and a dialog, collided. Pushing
//!   the node's own id here makes every widget id unique by construction,
//!   because normalization guarantees node ids are unique within a
//!   document and the caller scopes the whole document by slot.
//! - **`visible`/`enabled` are honored.** The flat element type has no
//!   way to express either, so the old renderer could not check them. The
//!   application layer already refuses to dispatch an action against a
//!   hidden or disabled node; drawing it as interactive anyway would show
//!   the user a control whose presses silently vanish.
//! - **`Split` renders one way.** The old stack handled `Split` four
//!   different ways across its call sites (two drew sidebar and content
//!   as flat sequential blocks with no separation, one used a real
//!   `SidePanel`, one called `flatten()` and discarded the distinction
//!   entirely). This always draws the real two-pane layout, which is what
//!   the WIT schema describes.
//!
//! # Events
//!
//! Rendering returns [`DocumentEvent`]s rather than invoking a callback,
//! matching this crate's MVU convention (render decides *what happened*;
//! a dispatcher decides what to do about it). A button press splits into
//! either host navigation or a plugin interaction -- never both, and
//! never a reserved event-id string; see
//! `crate::features::plugins::application::facade_sessions`'s module doc
//! comment for the encoding this replaces.

use arclain_app::plugins::{
    PluginActionDto, PluginImageDto, PluginKeyValueDto, PluginToolbarButtonDto, PluginUiDocument,
    PluginUiNodeDto, PluginUiNodeKind, PluginWarningIconDto,
};
use eframe::egui;

use super::image::ImageContext;
use crate::features::plugins::application::PluginNavigation;
use crate::shared::components::carousel::{Carousel, CarouselEvent};
use crate::shared::components::settings_form::{SectionHeader, SettingsGroup, SettingsRow};
use crate::shared::image_assets::{ImageAssetState, ImageOwner};
use crate::shared::theme::ThemeColors;
use crate::shared::SharedState;
use arclain_widgets::{Chips, TextInput, ThemedDropdown, ThemedSlider, ToggleSwitch};

/// One thing the user did to a document this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum DocumentEvent {
    /// A declarative button action the *host* resolves (open/close a
    /// dialog or page). Never forwarded to the plugin.
    Navigate(PluginNavigation),
    /// An interaction the plugin resolves, submitted through
    /// `ArclainApp::start_plugin_action`.
    Interact {
        node_id: String,
        action: PluginActionDto,
    },
}

impl DocumentEvent {
    fn activate(node_id: impl Into<String>) -> Self {
        Self::Interact {
            node_id: node_id.into(),
            action: PluginActionDto::Activate,
        }
    }

    fn set_value(node_id: impl Into<String>, value: impl Into<String>) -> Self {
        Self::Interact {
            node_id: node_id.into(),
            action: PluginActionDto::SetValue {
                value: Some(value.into()),
            },
        }
    }
}

/// Everything a document render needs beyond the document itself.
#[derive(Clone, Copy)]
pub struct DocumentContext<'a> {
    pub colors: &'a ThemeColors,
    pub shared_state: Option<&'a SharedState>,
    pub image_owner: Option<&'a ImageOwner>,
}

struct Sink<'a> {
    ctx: DocumentContext<'a>,
    plugin_id: &'a str,
    events: Vec<DocumentEvent>,
}

impl<'a> Sink<'a> {
    fn images(&self) -> ImageContext<'a> {
        ImageContext {
            shared_state: self.ctx.shared_state,
            plugin_id: Some(self.plugin_id),
            image_owner: self.ctx.image_owner,
        }
    }

    fn colors(&self) -> &'a ThemeColors {
        self.ctx.colors
    }
}

/// Renders `document`'s whole tree and returns everything the user did to
/// it this frame.
///
/// Marks the document's image owner active so
/// `ImageAssetStore::retain_owners` does not evict textures this document
/// is still drawing -- the same bookkeeping the flat renderer does.
pub fn render_document(
    ui: &mut egui::Ui,
    document: &PluginUiDocument,
    ctx: DocumentContext<'_>,
) -> Vec<DocumentEvent> {
    if let (Some(shared), Some(owner)) = (ctx.shared_state, ctx.image_owner) {
        shared.image_assets.mark_owner_active(owner.clone());
    }
    let mut sink = Sink {
        ctx,
        plugin_id: &document.plugin_id,
        events: Vec::new(),
    };
    // Scope the whole document by its session id: two slots rendering the
    // same plugin produce identical node ids (they are structural paths
    // and plugin-chosen element ids, both per-document), so without this
    // an open dialog and a panel showing the same plugin would collide on
    // every widget id.
    ui.push_id(document.session_id.into_raw(), |ui| {
        render_node(ui, &document.root, &mut sink);
    });
    sink.events
}

fn render_children(ui: &mut egui::Ui, children: &[PluginUiNodeDto], sink: &mut Sink<'_>) {
    for child in children {
        render_node(ui, child, sink);
    }
}

fn render_node(ui: &mut egui::Ui, node: &PluginUiNodeDto, sink: &mut Sink<'_>) {
    if !node.visible {
        return;
    }
    let enabled = node.enabled;
    ui.push_id(node.id.as_str(), |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            render_node_kind(ui, node, sink);
        });
    });
}

fn render_node_kind(ui: &mut egui::Ui, node: &PluginUiNodeDto, sink: &mut Sink<'_>) {
    let id = node.id.as_str();
    let colors = sink.colors();
    match &node.kind {
        PluginUiNodeKind::Single { children } => render_children(ui, children, sink),
        PluginUiNodeKind::Split {
            sidebar,
            content,
            sidebar_width,
        } => {
            egui::SidePanel::left(ui.id().with("split_sidebar"))
                .resizable(true)
                .default_width(sidebar_width.unwrap_or(250.0))
                .show_inside(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("split_sidebar_scroll")
                        .show(ui, |ui| render_children(ui, sidebar, sink));
                });
            egui::CentralPanel::default().show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("split_content_scroll")
                    .show(ui, |ui| render_children(ui, content, sink));
            });
        }
        PluginUiNodeKind::Group {
            title,
            description,
            children,
        } => {
            SettingsGroup::new(title)
                .content(|ui, group_colors| {
                    if let Some(description) = description {
                        ui.label(
                            egui::RichText::new(description)
                                .size(12.0)
                                .color(group_colors.on_surface_variant),
                        );
                        ui.add_space(6.0);
                    }
                    render_children(ui, children, sink);
                })
                .show(ui, colors);
        }
        PluginUiNodeKind::ListContainer {
            children,
            max_height,
            empty_message,
        } => {
            egui::Frame::NONE
                .fill(colors.surface_variant)
                .corner_radius(6.0)
                .inner_margin(4.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(max_height.unwrap_or(300.0))
                        .show(ui, |ui| {
                            if children.is_empty() {
                                let message = empty_message.as_deref().unwrap_or("No items");
                                ui.vertical_centered(|ui| {
                                    ui.add_space(40.0);
                                    ui.label(
                                        egui::RichText::new(message)
                                            .color(colors.on_surface_variant),
                                    );
                                    ui.add_space(40.0);
                                });
                            } else {
                                for child in children {
                                    render_node(ui, child, sink);
                                    ui.add_space(2.0);
                                }
                            }
                        });
                });
        }
        PluginUiNodeKind::Label { text, bold, size } => {
            if *bold && size.unwrap_or(14.0) >= 14.0 {
                SectionHeader::new(text).show(ui, colors);
            } else {
                let mut rich = egui::RichText::new(text).color(colors.on_surface);
                if *bold {
                    rich = rich.strong();
                }
                if let Some(size) = size {
                    rich = rich.size(*size);
                }
                ui.label(rich);
            }
        }
        PluginUiNodeKind::SectionHeader {
            title,
            level,
            description,
        } => {
            let mut header = SectionHeader::new(title).level(*level);
            if let Some(description) = description {
                header = header.description(description);
            }
            header.show(ui, colors);
        }
        PluginUiNodeKind::Separator => {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);
        }
        PluginUiNodeKind::Space { size } => ui.add_space(*size),
        PluginUiNodeKind::Button { label, action } => {
            if ui
                .add(
                    arclain_widgets::TextButton::new(label, arclain_widgets::ButtonSize::Small)
                        .with_theme_colors(colors),
                )
                .clicked()
            {
                let (navigation, interaction) = PluginNavigation::resolve(id, action.as_ref());
                if let Some(navigation) = navigation {
                    sink.events.push(DocumentEvent::Navigate(navigation));
                } else if let Some(node_id) = interaction {
                    sink.events.push(DocumentEvent::activate(node_id));
                }
            }
        }
        PluginUiNodeKind::TextInput {
            label,
            value,
            placeholder,
        } => render_text_input(ui, sink, id, label, value, placeholder.as_deref()),
        PluginUiNodeKind::Checkbox { label, checked } => {
            let temp_id = ui.make_persistent_id("checkbox");
            let mut is_checked = *checked;
            // Optimistic local state: a checkbox toggle round-trips through
            // a facade operation, so the incoming document still reports
            // the old value for a frame or two. Cleared as soon as the
            // document catches up.
            if let Some(optimistic) = ui.data(|data| data.get_temp::<bool>(temp_id)) {
                if optimistic == *checked {
                    ui.data_mut(|data| data.remove::<bool>(temp_id));
                } else {
                    is_checked = optimistic;
                }
            }
            let mut toggled = None;
            SettingsRow::new(label)
                .action(|ui| {
                    if ui.add(ToggleSwitch::new(&mut is_checked)).changed() {
                        ui.data_mut(|data| data.insert_temp(temp_id, is_checked));
                        toggled = Some(is_checked);
                    }
                })
                .show(ui, colors);
            if let Some(checked) = toggled {
                sink.events
                    .push(DocumentEvent::set_value(id, checked.to_string()));
            }
        }
        PluginUiNodeKind::RadioGroup {
            label,
            options,
            selected,
        } => {
            let mut chosen = None;
            SettingsRow::new(label)
                .action(|ui| {
                    let mut current = selected.clone();
                    ui.horizontal(|ui| {
                        for option in options {
                            if ui
                                .radio_value(
                                    &mut current,
                                    option.clone(),
                                    egui::RichText::new(option).color(colors.on_surface),
                                )
                                .changed()
                            {
                                chosen = Some(current.clone());
                            }
                        }
                    });
                })
                .show(ui, colors);
            if let Some(chosen) = chosen {
                sink.events.push(DocumentEvent::set_value(id, chosen));
            }
        }
        PluginUiNodeKind::Slider {
            label,
            value,
            min,
            max,
            step: _,
        } => {
            // `step` is intentionally unused: `ThemedSlider` is continuous
            // and the flat renderer ignored it too. Kept in the DTO so a
            // stepped slider can be wired without a schema change.
            let mut moved = None;
            SettingsRow::new(label)
                .action(|ui| {
                    let mut current = *value;
                    if ui
                        .add(ThemedSlider::new(&mut current, *min..=*max).with_theme_colors(colors))
                        .changed()
                    {
                        moved = Some(current);
                    }
                })
                .show(ui, colors);
            if let Some(value) = moved {
                sink.events
                    .push(DocumentEvent::set_value(id, f64::from(value).to_string()));
            }
        }
        PluginUiNodeKind::Dropdown {
            label,
            options,
            selected,
        } => {
            let mut chosen = None;
            SettingsRow::new(label)
                .action(|ui| {
                    let mut current = selected.clone();
                    ThemedDropdown::new(id, &current)
                        .with_theme_colors(colors)
                        .show_ui(ui, |ui| {
                            for option in options {
                                if ui
                                    .selectable_value(
                                        &mut current,
                                        option.clone(),
                                        egui::RichText::new(option).color(colors.on_surface),
                                    )
                                    .changed()
                                {
                                    chosen = Some(current.clone());
                                }
                            }
                        });
                })
                .show(ui, colors);
            if let Some(chosen) = chosen {
                sink.events.push(DocumentEvent::set_value(id, chosen));
            }
        }
        PluginUiNodeKind::Tabs { tabs, selected } => {
            let mut picked = None;
            egui::Frame::NONE
                .fill(colors.surface_variant)
                .corner_radius(8.0)
                .inner_margin(egui::Margin::symmetric(4, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for tab in tabs {
                            let is_selected = tab == selected;
                            let (fill, text) = if is_selected {
                                (colors.primary, colors.on_primary)
                            } else {
                                (egui::Color32::TRANSPARENT, colors.on_surface_variant)
                            };
                            let response = ui.add(
                                egui::Button::new(egui::RichText::new(tab).size(13.0).color(text))
                                    .fill(fill)
                                    .stroke(egui::Stroke::NONE)
                                    .corner_radius(6.0)
                                    .min_size(egui::vec2(0.0, 28.0)),
                            );
                            if response.clicked() {
                                picked = Some(tab.clone());
                            }
                            if response.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                        }
                    });
                });
            if let Some(tab) = picked {
                sink.events.push(DocumentEvent::set_value(id, tab));
            }
        }
        PluginUiNodeKind::Toolbar { buttons } => render_toolbar(ui, sink, buttons),
        PluginUiNodeKind::Warning { icon, message } => {
            egui::Frame::NONE
                .fill(colors.error.gamma_multiply(0.1))
                .stroke(egui::Stroke::new(1.0_f32, colors.error.gamma_multiply(0.3)))
                .inner_margin(8.0)
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(warning_glyph(icon))
                                .size(20.0)
                                .color(colors.error),
                        );
                        ui.label(egui::RichText::new(message).color(colors.on_surface));
                    });
                });
        }
        PluginUiNodeKind::Loading { message } => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().color(colors.primary));
                if let Some(message) = message {
                    ui.label(egui::RichText::new(message).color(colors.on_surface_variant));
                }
            });
        }
        PluginUiNodeKind::TagChips { tags, max_display } => {
            let display_count = max_display.map_or(tags.len(), |max| max as usize);
            let visible = &tags[..display_count.min(tags.len())];
            let remaining = tags.len().saturating_sub(display_count);
            ui.horizontal_wrapped(|ui| {
                for tag in visible {
                    ui.add(
                        Chips::new(tag)
                            .background_color(colors.primary.gamma_multiply(0.15))
                            .stroke_color(colors.primary.gamma_multiply(0.15))
                            .text_color(colors.primary),
                    );
                }
                if remaining > 0 {
                    ui.label(
                        egui::RichText::new(format!("+{remaining} more"))
                            .small()
                            .color(colors.on_surface_variant),
                    );
                }
            });
        }
        PluginUiNodeKind::Image {
            cache_key,
            url,
            max_height,
        } => render_image(ui, sink, cache_key.as_deref(), url.as_deref(), *max_height),
        PluginUiNodeKind::ListItem {
            title,
            subtitle,
            badge,
            image_key,
            image_url,
            selected,
            warning_icon,
        } => {
            let frame = if *selected {
                egui::Frame::NONE
                    .fill(colors.primary.gamma_multiply(0.15))
                    .inner_margin(8.0)
                    .corner_radius(4.0)
            } else {
                egui::Frame::NONE.inner_margin(8.0).corner_radius(4.0)
            };
            let images = sink.images();
            let response = frame
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if let Some(key) = image_key {
                            super::widgets::render_list_item_thumbnail(
                                ui, images, colors, key, image_url,
                            );
                        }
                        render_list_item_text(ui, colors, title, subtitle.as_deref());
                        render_list_item_meta(ui, colors, badge.as_deref(), warning_icon.as_ref());
                    });
                })
                .response;
            if response.interact(egui::Sense::click()).clicked() {
                sink.events.push(DocumentEvent::activate(id));
            }
        }
        PluginUiNodeKind::Carousel {
            images,
            current_index,
            max_height,
            thumbnail_height,
            enable_lightbox,
        } => render_carousel(
            ui,
            sink,
            id,
            images,
            *current_index,
            *max_height,
            *thumbnail_height,
            *enable_lightbox,
        ),
        PluginUiNodeKind::KeyValueList { items, columns } => {
            render_key_value_list(ui, colors, items, columns.unwrap_or(1) as usize);
        }
        PluginUiNodeKind::MetadataGrid { items, columns } => {
            render_metadata_grid(ui, colors, items, columns.unwrap_or(3) as usize);
        }
    }
}

fn warning_glyph(icon: &PluginWarningIconDto) -> &'static str {
    match icon {
        PluginWarningIconDto::Warning => egui_phosphor::regular::WARNING,
        PluginWarningIconDto::GlobeX => egui_phosphor::regular::GLOBE_X,
    }
}

fn render_text_input(
    ui: &mut egui::Ui,
    sink: &mut Sink<'_>,
    id: &str,
    label: &str,
    value: &str,
    placeholder: Option<&str>,
) {
    let colors = sink.colors();
    let temp_id = ui.make_persistent_id("text_input");
    let mut text = ui
        .data(|data| data.get_temp::<String>(temp_id))
        .unwrap_or_else(|| value.to_string());
    let mut submitted = None;

    if let Some(hint) = placeholder {
        // A hinted input is a filter box: submit on every change, the way
        // the flat renderer did, so the plugin can filter as the user
        // types.
        let response = TextInput::new(&mut text)
            .hint(hint)
            .width(ui.available_width())
            .with_theme_colors(colors)
            .show(ui);
        if response.changed() {
            ui.data_mut(|data| data.insert_temp(temp_id, text.clone()));
            submitted = Some(text.clone());
        }
    } else {
        let mut clear_temp = false;
        SettingsRow::new(label)
            .action(|ui| {
                ui.horizontal(|ui| {
                    let response = TextInput::new(&mut text)
                        .width(200.0)
                        .with_theme_colors(colors)
                        .show(ui);
                    if response.changed() {
                        ui.data_mut(|data| data.insert_temp(temp_id, text.clone()));
                    }
                    if text != *value {
                        let save = ui
                            .add(
                                arclain_widgets::TextButton::new(
                                    "Save",
                                    arclain_widgets::ButtonSize::Small,
                                )
                                .with_theme_colors(colors),
                            )
                            .clicked();
                        let entered = response.response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if save || entered {
                            submitted = Some(text.clone());
                            clear_temp = true;
                        }
                    }
                });
            })
            .show(ui, colors);
        if clear_temp {
            ui.data_mut(|data| data.remove::<String>(temp_id));
        }
    }

    if let Some(value) = submitted {
        sink.events.push(DocumentEvent::set_value(id, value));
    }
}

fn render_toolbar(ui: &mut egui::Ui, sink: &mut Sink<'_>, buttons: &[PluginToolbarButtonDto]) {
    let colors = sink.colors();
    let mut pressed = None;
    let make_button = |button: &PluginToolbarButtonDto| {
        arclain_widgets::TextButton::new(
            button.label.clone(),
            if button.primary {
                arclain_widgets::ButtonSize::Medium
            } else {
                arclain_widgets::ButtonSize::Small
            },
        )
        .with_theme_colors(colors)
    };
    ui.horizontal(|ui| {
        for button in buttons {
            if button.spacer_before {
                // Everything from the first spacer onward is right-aligned,
                // rendered in reverse so it reads left-to-right on screen --
                // the same arrangement the flat renderer produced.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for trailing in buttons.iter().rev().filter(|b| b.spacer_before) {
                        if ui.add(make_button(trailing)).clicked() {
                            pressed = Some(trailing.id.clone());
                        }
                    }
                });
                return;
            }
            if ui.add(make_button(button)).clicked() {
                pressed = Some(button.id.clone());
            }
        }
    });
    if let Some(node_id) = pressed {
        // A toolbar button is plain data inside a `Toolbar` node, not a
        // node of its own, so its id names nothing in the tree. The
        // application layer dispatches an unknown node id normally rather
        // than rejecting it -- see `PluginUiNodeDto::find`'s doc comment.
        sink.events.push(DocumentEvent::activate(node_id));
    }
}

fn render_image(
    ui: &mut egui::Ui,
    sink: &mut Sink<'_>,
    cache_key: Option<&str>,
    url: Option<&str>,
    max_height: Option<f32>,
) {
    let colors = sink.colors();
    let images = sink.images();
    let Some(key) = cache_key else {
        let message = url.map_or_else(
            || "🖼 [No image source]".to_string(),
            |url| format!("🖼 [Image: {url}]"),
        );
        ui.label(
            egui::RichText::new(message)
                .color(colors.on_surface_variant)
                .italics(),
        );
        return;
    };
    let (state, texture) = super::image::resolve_texture(ui, images, key);
    if let Some(texture) = texture {
        super::image::render_texture(ui, &texture, max_height);
        return;
    }
    if matches!(state, ImageAssetState::Failed(_)) {
        super::image::maybe_trigger_fetch(ui, images, key, url);
    }
    let (message, color) = match state {
        ImageAssetState::Failed(message) if url.is_none() => {
            (format!("🖼 [Error: {message}]"), colors.error)
        }
        ImageAssetState::Failed(_) => ("🖼 [Reloading...]".to_string(), colors.on_surface_variant),
        _ => (format!("🖼 [Loading: {key}]"), colors.on_surface_variant),
    };
    ui.label(egui::RichText::new(message).color(color).italics());
}

#[allow(clippy::too_many_arguments)]
fn render_carousel(
    ui: &mut egui::Ui,
    sink: &mut Sink<'_>,
    id: &str,
    images: &[PluginImageDto],
    current_index: u64,
    max_height: Option<f32>,
    thumbnail_height: Option<f32>,
    enable_lightbox: bool,
) {
    let pairs: Vec<(String, Option<String>)> = images
        .iter()
        .map(|image| (image.cache_key.clone(), image.url.clone()))
        .collect();
    let carousel = Carousel::new(id, &pairs, current_index as usize)
        .main_height(max_height.unwrap_or(300.0))
        .thumbnail_height(thumbnail_height.unwrap_or(60.0))
        .enable_lightbox(enable_lightbox)
        .colors(sink.colors())
        .shared_state(sink.ctx.shared_state)
        .plugin_id(Some(sink.plugin_id))
        .image_owner(sink.ctx.image_owner);

    // Carousel sub-interactions keep the flat renderer's `{id}_{verb}`
    // event-id convention verbatim: plugins parse these strings in their
    // own `on-ui-event` handlers, so changing them would break every
    // carousel-using plugin. They are plugin-facing ids, not host
    // navigation, so they go through the action channel like any other
    // interaction.
    if let Some(event) = carousel.show(ui) {
        let node_id = match event {
            CarouselEvent::Previous => format!("{id}_prev"),
            CarouselEvent::Next => format!("{id}_next"),
            CarouselEvent::Select(index) => format!("{id}_select_{index}"),
            CarouselEvent::OpenLightbox => format!("{id}_open_lightbox"),
        };
        sink.events.push(DocumentEvent::activate(node_id));
    }
}

fn render_list_item_text(
    ui: &mut egui::Ui,
    colors: &ThemeColors,
    title: &str,
    subtitle: Option<&str>,
) {
    ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
        ui.set_max_width(ui.available_width() - 80.0);
        ui.add(
            egui::Label::new(egui::RichText::new(title).strong().color(colors.on_surface))
                .truncate(),
        );
        if let Some(subtitle) = subtitle {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(subtitle)
                        .small()
                        .color(colors.on_surface_variant),
                )
                .truncate(),
            );
        }
    });
}

fn render_list_item_meta(
    ui: &mut egui::Ui,
    colors: &ThemeColors,
    badge: Option<&str>,
    warning_icon: Option<&PluginWarningIconDto>,
) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if let Some(icon) = warning_icon {
            ui.label(
                egui::RichText::new(warning_glyph(icon))
                    .size(16.0)
                    .color(colors.error),
            );
        }
        if let Some(badge) = badge {
            ui.label(
                egui::RichText::new(badge)
                    .small()
                    .color(colors.primary)
                    .background_color(colors.primary.gamma_multiply(0.1)),
            );
        }
    });
}

fn render_key_value_list(
    ui: &mut egui::Ui,
    colors: &ThemeColors,
    items: &[PluginKeyValueDto],
    columns: usize,
) {
    let columns = columns.max(1);
    // Salted off the ambient (node-scoped) id rather than a bare literal:
    // the flat renderer used `Grid::new("key_value_list")`, which is the
    // same egui id for every key-value list in the window.
    egui::Grid::new(ui.id().with("key_value_list"))
        .num_columns(columns * 2)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for (index, item) in items.iter().enumerate() {
                ui.label(
                    egui::RichText::new(&item.key)
                        .size(11.0)
                        .color(colors.on_surface_variant),
                );
                ui.label(
                    egui::RichText::new(&item.value)
                        .size(13.0)
                        .color(colors.on_surface),
                );
                if (index + 1) % columns == 0 {
                    ui.end_row();
                }
            }
        });
}

fn render_metadata_grid(
    ui: &mut egui::Ui,
    colors: &ThemeColors,
    items: &[PluginKeyValueDto],
    columns: usize,
) {
    let columns = columns.max(1);
    egui::Grid::new(ui.id().with("metadata_grid"))
        .num_columns(columns)
        .spacing([32.0, 8.0])
        .min_col_width(120.0)
        .show(ui, |ui| {
            for (index, item) in items.iter().enumerate() {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(item.key.to_uppercase())
                            .size(11.0)
                            .color(colors.on_surface_variant),
                    );
                    ui.label(
                        egui::RichText::new(&item.value)
                            .size(14.0)
                            .color(colors.on_surface),
                    );
                });
                if (index + 1) % columns == 0 {
                    ui.end_row();
                }
            }
        });
}
