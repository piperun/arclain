use crate::shared::components::settings_form::{SectionHeader, SettingsRow};
use crate::shared::theme::ThemeColors;
use arclain_data::ContentCache;
use arclain_plugins::types::PluginUiElement;
use eframe::egui;
use std::sync::Arc;

/// Callback for when a UI event occurs
pub type UiEventCallback<'a> = Box<dyn FnMut(&str, Option<String>) + 'a>;

/// Render a plugin UI element and its children
pub fn render_ui_element(
    ui: &mut egui::Ui,
    element: &PluginUiElement,
    event_callback: &mut UiEventCallback<'_>,
    colors: &ThemeColors,
    content_cache: Option<&Arc<ContentCache>>,
) {
    match element {
        PluginUiElement::Column { children, spacing } => {
            ui.vertical(|ui| {
                if let Some(sp) = spacing {
                    ui.spacing_mut().item_spacing.y = *sp;
                }
                for child in children {
                    render_ui_element(ui, child, event_callback, colors, content_cache);
                }
            });
        }
        PluginUiElement::Row { children, spacing } => {
            ui.horizontal(|ui| {
                if let Some(sp) = spacing {
                    ui.spacing_mut().item_spacing.x = *sp;
                }
                for child in children {
                    render_ui_element(ui, child, event_callback, colors, content_cache);
                }
            });
        }
        PluginUiElement::Grid { columns, children } => {
            egui::Grid::new("plugin_grid")
                .num_columns(*columns as usize)
                .show(ui, |ui| {
                    for (i, child) in children.iter().enumerate() {
                        render_ui_element(ui, child, event_callback, colors, content_cache);
                        if (i + 1) % (*columns as usize) == 0 {
                            ui.end_row();
                        }
                    }
                });
        }
        PluginUiElement::Label { text, bold, size } => {
            // Use SectionHeader if bold and large-ish, otherwise plain label
            if *bold && size.unwrap_or(14.0) >= 14.0 {
                SectionHeader::new(text).show(ui, colors);
            } else {
                let mut rich_text = egui::RichText::new(text).color(colors.on_surface);
                if *bold {
                    rich_text = rich_text.strong();
                }
                if let Some(s) = size {
                    rich_text = rich_text.size(*s);
                }
                ui.label(rich_text);
            }
        }
        PluginUiElement::Button { id, label, action } => {
            // Render button and handle action based on button_action field
            if ui
                .add(arclain_widgets::TextButton::new(
                    label,
                    arclain_widgets::ButtonSize::Small,
                ))
                .clicked()
            {
                match action
                    .as_ref()
                    .unwrap_or(&arclain_plugins::types::ButtonAction::None)
                {
                    arclain_plugins::types::ButtonAction::ShowDialog { id: dialog_id } => {
                        // Use special prefix to signal dialog open intent
                        event_callback(&format!("__dialog_open:{}", dialog_id), None);
                    }
                    arclain_plugins::types::ButtonAction::CloseDialog => {
                        event_callback("__dialog_close", None);
                    }
                    arclain_plugins::types::ButtonAction::OpenPage { id: page_id } => {
                        event_callback(&format!("__page_open:{}", page_id), None);
                    }
                    arclain_plugins::types::ButtonAction::ClosePage => {
                        event_callback("__page_close", None);
                    }
                    arclain_plugins::types::ButtonAction::Custom(custom_id) => {
                        event_callback(custom_id, None);
                    }
                    arclain_plugins::types::ButtonAction::None => {
                        // Normal button click - send to plugin
                        event_callback(id, None);
                    }
                }
            }
        }
        PluginUiElement::TextInput { id, label, value } => {
            let temp_id = ui.make_persistent_id(&id);
            // Retrieve temp state or default to current value
            let mut text = ui
                .data(|data| data.get_temp::<String>(temp_id))
                .unwrap_or(value.clone());

            SettingsRow::new(label)
                .action(|ui| {
                    ui.horizontal(|ui| {
                        let response =
                            ui.add(egui::TextEdit::singleline(&mut text).desired_width(200.0));

                        // If changed, update temp state
                        if response.changed() {
                            ui.data_mut(|data| data.insert_temp(temp_id, text.clone()));
                        }

                        // Show Save button if text differs from stored value
                        let is_modified = text != *value;
                        if is_modified {
                            if ui.button("Save").clicked()
                                || (response.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            {
                                event_callback(id, Some(text.clone()));
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
        PluginUiElement::Checkbox { id, label, checked } => {
            let temp_id = ui.make_persistent_id(id);
            let mut is_checked = *checked;

            // Check for optimistic state to handle thread latency
            if let Some(optimistic) = ui.data(|d| d.get_temp::<bool>(temp_id)) {
                if optimistic == *checked {
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
                        event_callback(id, Some(is_checked.to_string()));
                    }
                })
                .show(ui, colors);
        }
        PluginUiElement::Separator => {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);
        }
        PluginUiElement::Space { size } => {
            ui.add_space(*size);
        }
        PluginUiElement::RadioGroup {
            id,
            label,
            options,
            selected,
        } => {
            SettingsRow::new(label)
                .action(|ui| {
                    let mut current_selected = selected.clone();
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
                        event_callback(id, Some(current_selected));
                    }
                })
                .show(ui, colors);
        }
        PluginUiElement::Slider {
            id,
            label,
            value,
            min,
            max,
            step,
        } => {
            SettingsRow::new(label)
                .action(|ui| {
                    let mut current_value = *value;
                    let slider = egui::Slider::new(&mut current_value, *min..=*max);
                    let slider = if let Some(s) = step {
                        slider.step_by(*s as f64)
                    } else {
                        slider
                    };

                    if ui.add(slider).changed() {
                        event_callback(id, Some(current_value.to_string()));
                    }
                })
                .show(ui, colors);
        }
        PluginUiElement::Dropdown {
            id,
            label,
            options,
            selected,
        } => {
            SettingsRow::new(label)
                .action(|ui| {
                    let mut current_selected = selected.clone();
                    egui::ComboBox::from_id_salt(id)
                        .selected_text(
                            egui::RichText::new(&current_selected).color(colors.on_surface),
                        )
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
                                    event_callback(id, Some(current_selected.clone()));
                                }
                            }
                        });
                })
                .show(ui, colors);
        }
        PluginUiElement::Image {
            cache_key,
            url,
            max_height,
        } => {
            // Try to load image from cache
            if let Some(key) = cache_key {
                if let Some(cache) = content_cache {
                    match cache.get(key) {
                        Ok(Some(bytes)) => {
                            // Try to decode image and display
                            if let Some(size) = try_render_image(ui, key, &bytes, *max_height) {
                                // Successfully rendered
                                let _ = size;
                            } else {
                                // Failed to decode
                                ui.label(
                                    egui::RichText::new("🖼 [Invalid image data]")
                                        .color(colors.on_surface_variant)
                                        .italics(),
                                );
                            }
                        }
                        Ok(None) => {
                            // Not in cache yet
                            ui.label(
                                egui::RichText::new(format!("🖼 [Loading: {}]", key))
                                    .color(colors.on_surface_variant)
                                    .italics(),
                            );
                        }
                        Err(e) => {
                            ui.label(
                                egui::RichText::new(format!("🖼 [Error: {}]", e))
                                    .color(colors.error)
                                    .italics(),
                            );
                        }
                    }
                } else {
                    ui.label(
                        egui::RichText::new("🖼 [No cache available]")
                            .color(colors.on_surface_variant)
                            .italics(),
                    );
                }
            } else if let Some(url_str) = url {
                // URL without cache key - show placeholder
                ui.label(
                    egui::RichText::new(format!("🖼 [Image: {}]", url_str))
                        .color(colors.on_surface_variant)
                        .italics(),
                );
            } else {
                ui.label(
                    egui::RichText::new("🖼 [No image source]")
                        .color(colors.on_surface_variant)
                        .italics(),
                );
            }
        }
        PluginUiElement::Tabs { id, tabs, selected } => {
            ui.horizontal(|ui| {
                for tab in tabs {
                    let is_selected = tab == selected;
                    let style = if is_selected {
                        egui::RichText::new(tab).strong().color(colors.primary)
                    } else {
                        egui::RichText::new(tab).color(colors.on_surface_variant)
                    };

                    if ui.selectable_label(is_selected, style).clicked() {
                        event_callback(id, Some(tab.clone()));
                    }
                }
            });
        }
        PluginUiElement::ListItem {
            id,
            title,
            subtitle,
            badge,
            image_key,
            selected,
        } => {
            let frame = if *selected {
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
                            if let Some(cache) = content_cache {
                                if let Ok(Some(bytes)) = cache.get(key) {
                                    // Small thumbnail (48x48)
                                    if let Some(_size) =
                                        try_render_image(ui, key, &bytes, Some(48.0))
                                    {
                                        // rendered
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
                event_callback(id, None);
            }
        }
        PluginUiElement::ListContainer {
            id: _,
            items,
            max_height,
            empty_message,
        } => {
            let height = max_height.unwrap_or(300.0);

            egui::Frame::NONE
                .fill(colors.surface_variant)
                .corner_radius(6.0)
                .inner_margin(4.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(height)
                        .show(ui, |ui| {
                            if items.is_empty() {
                                let msg = empty_message.as_deref().unwrap_or("No items");
                                ui.vertical_centered(|ui| {
                                    ui.add_space(40.0);
                                    ui.label(
                                        egui::RichText::new(msg).color(colors.on_surface_variant),
                                    );
                                    ui.add_space(40.0);
                                });
                            } else {
                                for item in items {
                                    render_ui_element(
                                        ui,
                                        item,
                                        event_callback,
                                        colors,
                                        content_cache,
                                    );
                                    ui.add_space(2.0);
                                }
                            }
                        });
                });
        }
        PluginUiElement::Loading { message } => {
            ui.horizontal(|ui| {
                ui.spinner();
                if let Some(msg) = message {
                    ui.label(egui::RichText::new(msg).color(colors.on_surface_variant));
                }
            });
        }
    }
}

/// Render a list of UI elements
pub fn render_ui_elements(
    ui: &mut egui::Ui,
    elements: &[PluginUiElement],
    event_callback: &mut UiEventCallback<'_>,
    colors: &ThemeColors,
    content_cache: Option<&Arc<ContentCache>>,
) {
    for element in elements {
        render_ui_element(ui, element, event_callback, colors, content_cache);
    }
}

/// Try to render an image from raw bytes
/// Returns the displayed size if successful, None if decoding failed
fn try_render_image(
    ui: &mut egui::Ui,
    cache_key: &str,
    bytes: &[u8],
    max_height: Option<f32>,
) -> Option<egui::Vec2> {
    let ctx = ui.ctx();

    // Generate a stable ID for this image's texture
    let texture_id = egui::Id::new(("plugin_image", cache_key));

    // Check if texture is already loaded in egui's memory
    let existing_handle: Option<egui::TextureHandle> = ctx.data(|d| d.get_temp(texture_id));

    let handle = if let Some(h) = existing_handle {
        h
    } else {
        // Try to decode the image bytes using the image crate
        let img = image::load_from_memory(bytes).ok()?;
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let pixels = rgba.into_raw();

        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);

        // Load into egui
        let handle = ctx.load_texture(cache_key, color_image, egui::TextureOptions::default());

        // Cache the handle for future frames
        ctx.data_mut(|d| d.insert_temp(texture_id, handle.clone()));

        handle
    };

    // Calculate display size respecting max_height
    let tex_size = handle.size_vec2();
    let max_h = max_height.unwrap_or(200.0);
    let scale = if tex_size.y > max_h {
        max_h / tex_size.y
    } else {
        1.0
    };
    let display_size = egui::vec2(tex_size.x * scale, tex_size.y * scale);

    // Render the image
    ui.image(egui::load::SizedTexture {
        id: handle.id(),
        size: display_size,
    });

    Some(display_size)
}
