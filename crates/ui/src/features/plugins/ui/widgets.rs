//! Basic widgets for plugins

use super::context::{RenderContext, UiEventHandler};
use super::image::{trigger_image_fetch, try_render_image};
use crate::shared::components::settings_form::{SectionHeader, SettingsRow};
use arclain_plugins::types::{ButtonAction, WarningIcon};
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
    if ui
        .add(arclain_widgets::TextButton::new(
            label,
            arclain_widgets::ButtonSize::Small,
        ))
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
) {
    let colors = ctx.colors;
    let temp_id = ui.make_persistent_id(id);
    // Retrieve temp state or default to current value
    let mut text = ui
        .data(|data| data.get_temp::<String>(temp_id))
        .unwrap_or(value.to_string());

    SettingsRow::new(label)
        .action(|ui| {
            ui.horizontal(|ui| {
                let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(200.0));

                // If changed, update temp state
                if response.changed() {
                    ui.data_mut(|data| data.insert_temp(temp_id, text.clone()));
                }

                // Show Save button if text differs from stored value
                let is_modified = text != *value;
                if is_modified {
                    if ui.button("Save").clicked()
                        || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        (ctx.event_callback)(id, Some(text.clone()));
                        // Clear temp state to sync with new incoming value
                        ui.data_mut(|data| data.remove::<String>(temp_id));
                    }
                } else if response.lost_focus() {
                    // If focus lost without changes (or reverted), assume sync
                    // Optional: clear temp logic if needed
                }
            });
        })
        .show(ui, colors);
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
            egui::ComboBox::from_id_salt(id)
                .selected_text(egui::RichText::new(&current_selected).color(colors.on_surface))
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
    ui.horizontal(|ui| {
        for tab in tabs {
            let is_selected = tab == selected;
            let style = if is_selected {
                egui::RichText::new(tab).strong().color(colors.primary)
            } else {
                egui::RichText::new(tab).color(colors.on_surface_variant)
            };

            if ui.selectable_label(is_selected, style).clicked() {
                (ctx.event_callback)(id, Some(tab.clone()));
            }
        }
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
        ui.spinner();
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
    ui.horizontal(|ui| {
        for btn in buttons {
            // Use different styling for primary buttons
            let button = arclain_widgets::TextButton::new(
                &btn.label,
                if btn.primary {
                    arclain_widgets::ButtonSize::Medium
                } else {
                    arclain_widgets::ButtonSize::Small
                },
            );

            if ui.add(button).clicked() {
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
                            if let Some(_size) = try_render_image(ui, key, &bytes, Some(48.0)) {
                                // rendered
                            }
                        } else {
                            // Placeholder & Fetch
                            ui.add(egui::Spinner::new().size(16.0));

                            if let Some(shared) = ctx.shared_state {
                                let fetch_id = egui::Id::new(("fetch", key.as_str()));
                                let fetching: bool =
                                    ui.data(|d| d.get_temp(fetch_id)).unwrap_or(false);
                                if !fetching {
                                    ui.data_mut(|d| d.insert_temp(fetch_id, true));
                                    trigger_image_fetch(
                                        shared,
                                        ctx.plugin_id.map(|s| s.to_string()),
                                        key.clone(),
                                        key.clone(),
                                        ui.ctx().clone(),
                                    );
                                }
                            }
                        }
                    }
                }

                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(title).strong().color(colors.on_surface));
                    if let Some(sub) = subtitle {
                        ui.label(
                            egui::RichText::new(sub)
                                .small()
                                .color(colors.on_surface_variant),
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
