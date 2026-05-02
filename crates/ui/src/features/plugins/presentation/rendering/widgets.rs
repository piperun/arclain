//! Basic widgets for plugins

use super::context::{RenderContext, UiEventHandler};
use super::image::{trigger_image_fetch, try_render_image};
use crate::shared::components::settings_form::{SectionHeader, SettingsGroup, SettingsRow};
use arclain_plugins::types::{ButtonAction, PluginUiElement, WarningIcon};
use arclain_widgets::{TextInput, ThemedDropdown};
use eframe::egui;

pub fn render_label(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    text: &str,
    bold: bool,
    size: Option<f32>,
) {
    let colors = ctx.colors;
    // Use SectionHeader if bold and large-ish, otherwise plain label
    if bold && size.unwrap_or(14.0) >= 14.0 {
        SectionHeader::new(text).show(ui, colors);
    } else {
        let mut rich_text = egui::RichText::new(text).color(colors.on_surface);
        if bold {
            rich_text = rich_text.strong();
        }
        if let Some(s) = size {
            rich_text = rich_text.size(s);
        }
        ui.label(rich_text);
    }
}

pub fn render_button(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    action: &Option<ButtonAction>,
) {
    let colors = ctx.colors;
    if ui
        .add(
            arclain_widgets::TextButton::new(label, arclain_widgets::ButtonSize::Small)
                .with_theme_colors(colors),
        )
        .clicked()
    {
        match action.as_ref().unwrap_or(&ButtonAction::None) {
            ButtonAction::ShowDialog { id: dialog_id } => {
                // Use special prefix to signal dialog open intent
                (ctx.event_callback)(&format!("__dialog_open:{}", dialog_id), None);
            }
            ButtonAction::CloseDialog => {
                (ctx.event_callback)("__dialog_close", None);
            }
            ButtonAction::OpenPage { id: page_id } => {
                (ctx.event_callback)(&format!("__page_open:{}", page_id), None);
            }
            ButtonAction::ClosePage => {
                (ctx.event_callback)("__page_close", None);
            }
            ButtonAction::Custom(custom_id) => {
                (ctx.event_callback)(custom_id, None);
            }
            ButtonAction::None => {
                // Normal button click - send to plugin
                (ctx.event_callback)(id, None);
            }
        }
    }
}

pub fn render_text_input(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    value: &str,
    placeholder: &Option<String>,
) {
    let colors = ctx.colors;
    let temp_id = ui.make_persistent_id(id);
    // Retrieve temp state or default to current value
    let mut text = ui
        .data(|data| data.get_temp::<String>(temp_id))
        .unwrap_or(value.to_string());

    // If placeholder is set, render as simple search-style input (no label title)
    if let Some(hint) = placeholder {
        let response = TextInput::new(&mut text)
            .hint(hint)
            .width(ui.available_width())
            .with_theme_colors(colors)
            .show(ui);

        if response.changed() {
            ui.data_mut(|data| data.insert_temp(temp_id, text.clone()));
            // Auto-submit on change for filter inputs
            (ctx.event_callback)(id, Some(text.clone()));
        }
    } else {
        // Original behavior with SettingsRow wrapper
        SettingsRow::new(label)
            .action(|ui| {
                ui.horizontal(|ui| {
                    let response = TextInput::new(&mut text)
                        .width(200.0)
                        .with_theme_colors(colors)
                        .show(ui);

                    // If changed, update temp state
                    if response.changed() {
                        ui.data_mut(|data| data.insert_temp(temp_id, text.clone()));
                    }

                    // Show Save button if text differs from stored value
                    let is_modified = text != *value;
                    if is_modified {
                        if ui.add(arclain_widgets::TextButton::new("Save", arclain_widgets::ButtonSize::Small).with_theme_colors(colors)).clicked()
                            || (response.response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            (ctx.event_callback)(id, Some(text.clone()));
                            // Clear temp state to sync with new incoming value
                            ui.data_mut(|data| data.remove::<String>(temp_id));
                        }
                    } else if response.response.lost_focus() {
                        // If focus lost without changes (or reverted), assume sync
                        // Optional: clear temp logic if needed
                    }
                });
            })
            .show(ui, colors);
    }
}

pub fn render_checkbox(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    checked: bool,
) {
    let colors = ctx.colors;
    let temp_id = ui.make_persistent_id(id);
    let mut is_checked = checked;

    // Check for optimistic state to handle thread latency
    if let Some(optimistic) = ui.data(|d| d.get_temp::<bool>(temp_id)) {
        if optimistic == checked {
            // Backend has caught up, clear optimistic state
            ui.data_mut(|d| d.remove::<bool>(temp_id));
        } else {
            // Backend stale, use optimistic value
            is_checked = optimistic;
        }
    }

    SettingsRow::new(label)
        .action(|ui| {
            if ui
                .add(arclain_widgets::ToggleSwitch::new(&mut is_checked))
                .changed()
            {
                // Set optimistic state immediately
                ui.data_mut(|d| d.insert_temp(temp_id, is_checked));
                (ctx.event_callback)(id, Some(is_checked.to_string()));
            }
        })
        .show(ui, colors);
}

pub fn render_radio_group(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    options: &[String],
    selected: &str,
) {
    let colors = ctx.colors;
    SettingsRow::new(label)
        .action(|ui| {
            let mut current_selected = selected.to_string();
            let mut changed = false;
            ui.horizontal(|ui| {
                for option in options {
                    if ui
                        .radio_value(
                            &mut current_selected,
                            option.clone(),
                            egui::RichText::new(option).color(colors.on_surface),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                }
            });
            if changed {
                (ctx.event_callback)(id, Some(current_selected));
            }
        })
        .show(ui, colors);
}

pub fn render_slider(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    value: f64,
    min: f64,
    max: f64,
    step: Option<f64>,
) {
    let colors = ctx.colors;
    SettingsRow::new(label)
        .action(|ui| {
            let mut current_value = value;
            let slider = egui::Slider::new(&mut current_value, min..=max);
            let slider = if let Some(s) = step {
                slider.step_by(s)
            } else {
                slider
            };

            if ui.add(slider).changed() {
                (ctx.event_callback)(id, Some(current_value.to_string()));
            }
        })
        .show(ui, colors);
}

pub fn render_dropdown(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    options: &[String],
    selected: &str,
) {
    let colors = ctx.colors;
    SettingsRow::new(label)
        .action(|ui| {
            let mut current_selected = selected.to_string();
            ThemedDropdown::new(id, &current_selected)
                .with_theme_colors(colors)
                .show_ui(ui, |ui| {
                    for option in options {
                        if ui
                            .selectable_value(
                                &mut current_selected,
                                option.clone(),
                                egui::RichText::new(option).color(colors.on_surface),
                            )
                            .changed()
                        {
                            (ctx.event_callback)(id, Some(current_selected.clone()));
                        }
                    }
                });
        })
        .show(ui, colors);
}

pub fn render_tabs(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    tabs: &[String],
    selected: &str,
) {
    let colors = ctx.colors;

    // Pill-style container
    egui::Frame::NONE
        .fill(colors.surface_variant)
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(4, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;

                for tab in tabs {
                    let is_selected = tab == selected;

                    // Tab button styling
                    let (bg_color, text_color) = if is_selected {
                        (colors.primary, colors.on_primary)
                    } else {
                        (egui::Color32::TRANSPARENT, colors.on_surface_variant)
                    };

                    let button = egui::Button::new(
                        egui::RichText::new(tab).size(13.0).color(text_color),
                    )
                    .fill(bg_color)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(6.0)
                    .min_size(egui::vec2(0.0, 28.0));

                    let response = ui.add(button);

                    // Hover effect for non-selected tabs
                    if !is_selected && response.hovered() {
                        let hover_rect = response.rect;
                        ui.painter().rect_filled(
                            hover_rect,
                            6.0,
                            colors.on_surface.gamma_multiply(0.08),
                        );
                    }

                    if response.clicked() {
                        (ctx.event_callback)(id, Some(tab.clone()));
                    }

                    // Pointer cursor on hover
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
            });
        });
}

pub fn render_warning(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    icon: &WarningIcon,
    message: &str,
) {
    let colors = ctx.colors;
    let bg_color = colors.error.gamma_multiply(0.1);
    let stroke_color = colors.error.gamma_multiply(0.3);

    egui::Frame::NONE
        .fill(bg_color)
        .stroke(egui::Stroke::new(1.0, stroke_color))
        .inner_margin(8.0)
        .corner_radius(4.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let icon_str = match icon {
                    WarningIcon::Warning => egui_phosphor::regular::WARNING,
                    WarningIcon::GlobeX => egui_phosphor::regular::GLOBE_X,
                };

                ui.label(egui::RichText::new(icon_str).size(20.0).color(colors.error));

                ui.label(egui::RichText::new(message).color(colors.on_surface));
            });
        });
}

pub fn render_loading(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    message: &Option<String>,
) {
    let colors = ctx.colors;
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().color(colors.primary));
        if let Some(msg) = message {
            ui.label(egui::RichText::new(msg).color(colors.on_surface_variant));
        }
    });
}

pub fn render_tag_chips(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    tags: &[String],
    max_display: Option<u32>,
) {
    let colors = ctx.colors;
    let display_count = max_display.map(|m| m as usize).unwrap_or(tags.len());
    let visible_tags = &tags[..display_count.min(tags.len())];
    let remaining = tags.len().saturating_sub(display_count);

    ui.horizontal_wrapped(|ui| {
        for tag in visible_tags {
            // Chip-style tag button
            let chip_frame = egui::Frame::NONE
                .fill(colors.primary.gamma_multiply(0.15))
                .inner_margin(egui::Margin::symmetric(8, 4))
                .corner_radius(12.0);

            chip_frame.show(ui, |ui| {
                ui.label(egui::RichText::new(tag).small().color(colors.primary));
            });
        }

        if remaining > 0 {
            ui.label(
                egui::RichText::new(format!("+{} more", remaining))
                    .small()
                    .color(colors.on_surface_variant),
            );
        }
    });
}

pub fn render_toolbar(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    buttons: &[arclain_plugins::types::ToolbarButton],
) {
    let colors = ctx.colors;
    let make_button = |label: &str, primary: bool| {
        arclain_widgets::TextButton::new(
            label.to_string(),
            if primary {
                arclain_widgets::ButtonSize::Medium
            } else {
                arclain_widgets::ButtonSize::Small
            },
        )
        .with_theme_colors(colors)
    };
    ui.horizontal(|ui| {
        for btn in buttons {
            // Add flexible space before this button if requested
            if btn.spacer_before {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Render remaining buttons right-to-left
                    for rbtn in buttons.iter().rev() {
                        if !rbtn.spacer_before {
                            continue; // Skip buttons before spacer
                        }

                        if ui.add(make_button(&rbtn.label, rbtn.primary)).clicked() {
                            (ctx.event_callback)(&rbtn.id, None);
                        }
                    }
                });
                return; // Done rendering
            }

            if ui.add(make_button(&btn.label, btn.primary)).clicked() {
                (ctx.event_callback)(&btn.id, None);
            }
        }
    });
}

pub fn render_list_item(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    title: &str,
    subtitle: &Option<String>,
    badge: &Option<String>,
    image_key: &Option<String>,
    image_url: &Option<String>,
    selected: bool,
    warning_icon: &Option<WarningIcon>,
) {
    let colors = ctx.colors;
    let frame = if selected {
        egui::Frame::NONE
            .fill(colors.primary.gamma_multiply(0.15))
            .inner_margin(8.0)
            .corner_radius(4.0)
    } else {
        egui::Frame::NONE.inner_margin(8.0).corner_radius(4.0)
    };

    let response = frame
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Optional thumbnail
                if let Some(key) = image_key {
                    if let Some(cache) = ctx.content_cache {
                        if let Ok(Some(bytes)) = cache.get(key) {
                            // Small thumbnail (48x48)
                            if try_render_image(ui, key, &bytes, Some(48.0)).is_none() {
                                // Image decode failed - delete bad cache entry and show spinner
                                tracing::debug!("Deleting corrupt cache entry: {}", key);
                                let _ = cache.remove(key);
                                ui.add(egui::Spinner::new().size(16.0).color(colors.primary));

                                // Trigger re-fetch
                                if let Some(url) = image_url {
                                    if let Some(shared) = ctx.shared_state {
                                        trigger_image_fetch(
                                            shared,
                                            ctx.plugin_id.map(|s| s.to_string()),
                                            url.clone(),
                                            key.clone(),
                                            ui.ctx().clone(),
                                        );
                                    }
                                }
                            }
                        } else {
                            // Placeholder & Fetch - use primary color for visibility
                            ui.add(egui::Spinner::new().size(16.0).color(colors.primary));

                            // Only fetch if we have a URL to fetch from
                            if let Some(url) = image_url {
                                if let Some(shared) = ctx.shared_state {
                                    // Use timestamp to allow retry after 30 seconds if fetch failed
                                    let fetch_id = egui::Id::new(("fetch", key.as_str()));
                                    let now = std::time::Instant::now();
                                    let fetch_started: Option<std::time::Instant> =
                                        ui.data(|d| d.get_temp(fetch_id));

                                    let should_fetch = match fetch_started {
                                        None => true,
                                        Some(started) => {
                                            now.duration_since(started).as_secs() > 30
                                        }
                                    };

                                    if should_fetch {
                                        ui.data_mut(|d| d.insert_temp(fetch_id, now));
                                        trigger_image_fetch(
                                            shared,
                                            ctx.plugin_id.map(|s| s.to_string()),
                                            url.clone(),
                                            key.clone(),
                                            ui.ctx().clone(),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                // Text content with truncation (ellipsis) to prevent width overflow
                // Set max width to allow truncation to work
                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(ui.available_width() - 80.0); // Leave space for badge/icon

                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(title).strong().color(colors.on_surface),
                        )
                        .truncate(),
                    );
                    if let Some(sub) = subtitle {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(sub)
                                    .small()
                                    .color(colors.on_surface_variant),
                            )
                            .truncate(),
                        );
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Warning icon (if present)
                    if let Some(icon) = warning_icon {
                        let icon_str = match icon {
                            WarningIcon::Warning => egui_phosphor::regular::WARNING,
                            WarningIcon::GlobeX => egui_phosphor::regular::GLOBE_X,
                        };
                        ui.label(egui::RichText::new(icon_str).size(16.0).color(colors.error));
                    }

                    if let Some(badge_text) = badge {
                        ui.label(
                            egui::RichText::new(badge_text)
                                .small()
                                .color(colors.primary)
                                .background_color(colors.primary.gamma_multiply(0.1)),
                        );
                    }
                });
            });
        })
        .response;

    if response.interact(egui::Sense::click()).clicked() {
        (ctx.event_callback)(id, None);
    }
}

/// Render a visually-grouped settings section (matches the host's
/// `Form/SettingsGroup` styling). The `walk_body` callback receives a
/// per-frame `Ui` plus the inner elements and is expected to render them —
/// typically by recursing through `walk_with_groups` so nested
/// `GroupBegin`/`GroupEnd` pairs work.
pub fn render_settings_group<H, F>(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, H>,
    title: &str,
    description: &Option<String>,
    body: &[PluginUiElement],
    mut walk_body: F,
) where
    H: UiEventHandler + ?Sized,
    F: FnMut(&mut egui::Ui, &[PluginUiElement], &mut RenderContext<'_, H>),
{
    let colors = ctx.colors;
    let description = description.clone();
    SettingsGroup::new(title)
        .content(|ui, group_colors| {
            if let Some(desc) = description {
                ui.label(
                    egui::RichText::new(desc)
                        .size(12.0)
                        .color(group_colors.on_surface_variant),
                );
                ui.add_space(6.0);
            }
            walk_body(ui, body, ctx);
        })
        .show(ui, colors);
}

/// Render a section header with semantic title hierarchy (h1-h4 style)
/// Uses the shared SectionHeader component for consistency
pub fn render_section_header(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    title: &str,
    level: u32,
    description: &Option<String>,
) {
    let mut header = SectionHeader::new(title).level(level);
    if let Some(desc) = description {
        header = header.description(desc);
    }
    header.show(ui, ctx.colors);
}

/// Render a key-value list as a two-column grid for metadata display
pub fn render_key_value_list(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    items: &[arclain_plugins::types::KeyValuePair],
    columns: Option<u32>,
) {
    let colors = ctx.colors;
    let cols = columns.unwrap_or(1) as usize;

    // Calculate number of data columns (each key-value pair = 2 columns)
    let grid_columns = cols * 2;

    egui::Grid::new("key_value_list")
        .num_columns(grid_columns)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for (i, kv) in items.iter().enumerate() {
                // Key label (muted, smaller)
                ui.label(
                    egui::RichText::new(&kv.key)
                        .size(11.0)
                        .color(colors.on_surface_variant),
                );
                // Value
                ui.label(
                    egui::RichText::new(&kv.value)
                        .size(13.0)
                        .color(colors.on_surface),
                );

                // End row after the specified number of columns
                if (i + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
}

/// Render a metadata grid with label above value (card-style layout)
/// This matches the mockup style: uppercase labels, larger values below
pub fn render_metadata_grid(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    items: &[arclain_plugins::types::KeyValuePair],
    columns: Option<u32>,
) {
    let colors = ctx.colors;
    let cols = columns.unwrap_or(3) as usize;

    // Use Grid for proper column alignment
    egui::Grid::new("metadata_grid")
        .num_columns(cols)
        .spacing([32.0, 8.0])
        .min_col_width(120.0)
        .show(ui, |ui| {
            for (i, kv) in items.iter().enumerate() {
                // Each cell is a vertical stack: label on top, value below
                ui.vertical(|ui| {
                    // Label (uppercase, muted, smaller)
                    ui.label(
                        egui::RichText::new(kv.key.to_uppercase())
                            .size(11.0)
                            .color(colors.on_surface_variant),
                    );
                    // Value (larger, primary color)
                    ui.label(
                        egui::RichText::new(&kv.value)
                            .size(14.0)
                            .color(colors.on_surface),
                    );
                });

                // End row after the specified number of columns
                if (i + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
}
