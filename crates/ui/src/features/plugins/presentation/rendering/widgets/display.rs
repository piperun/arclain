//! Read-only / display-only widgets — they render data the plugin
//! provides but don't take direct user input. List items emit a click
//! event for selection, but the cell values themselves aren't editable.

use super::super::context::{RenderContext, UiEventHandler};
use super::super::image::{trigger_image_fetch, try_render_image};
use crate::shared::components::settings_form::SectionHeader;
use arclain_plugins::types::WarningIcon;
use arclain_widgets::Chips;
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
            ui.add(
                Chips::new(tag)
                    .background_color(colors.primary.gamma_multiply(0.15))
                    .stroke_color(colors.primary.gamma_multiply(0.15))
                    .text_color(colors.primary),
            );
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

#[allow(clippy::too_many_arguments)]
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
                if let Some(key) = image_key {
                    render_list_item_thumbnail(ui, ctx, key, image_url);
                }
                render_list_item_text(ui, colors, title, subtitle);
                render_list_item_meta(ui, colors, badge, warning_icon);
            });
        })
        .response;

    if response.interact(egui::Sense::click()).clicked() {
        (ctx.event_callback)(id, None);
    }
}

/// Re-fetch a failed image at most every this often. Avoids hammering
/// a flaky CDN every frame for an image that just isn't available.
const IMAGE_FETCH_RETRY_INTERVAL_SECS: u64 = 30;

/// Render a 48px-square thumbnail tied to a content-cache key.
///
/// Three states:
/// * cache hit + decode ok → image is drawn.
/// * cache hit + decode fail → drop the corrupt entry, draw a spinner,
///   and re-trigger the fetch (cache poisoning recovery).
/// * cache miss → draw a spinner and trigger a fetch the first time, or
///   after `IMAGE_FETCH_RETRY_INTERVAL_SECS` have passed since the last
///   attempt.
fn render_list_item_thumbnail(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    key: &str,
    image_url: &Option<String>,
) {
    let Some(cache) = ctx.content_cache else {
        return;
    };
    let colors = ctx.colors;

    match cache.get(key) {
        Ok(Some(bytes)) => {
            if try_render_image(ui, key, &bytes, Some(48.0)).is_none() {
                // Decode failed — drop the bad entry and re-trigger.
                tracing::debug!("Deleting corrupt cache entry: {}", key);
                let _ = cache.remove(key);
                ui.add(egui::Spinner::new().size(16.0).color(colors.primary));
                if let (Some(url), Some(shared)) = (image_url, ctx.shared_state) {
                    trigger_image_fetch(
                        shared,
                        ctx.plugin_id.map(|s| s.to_string()),
                        url.clone(),
                        key.to_string(),
                        ui.ctx().clone(),
                    );
                }
            }
        }
        _ => {
            ui.add(egui::Spinner::new().size(16.0).color(colors.primary));
            if let (Some(url), Some(shared)) = (image_url, ctx.shared_state) {
                let fetch_id = egui::Id::new(("fetch", key));
                let now = std::time::Instant::now();
                let fetch_started: Option<std::time::Instant> =
                    ui.data(|d| d.get_temp(fetch_id));

                let should_fetch = match fetch_started {
                    None => true,
                    Some(started) => {
                        now.duration_since(started).as_secs()
                            > IMAGE_FETCH_RETRY_INTERVAL_SECS
                    }
                };

                if should_fetch {
                    ui.data_mut(|d| d.insert_temp(fetch_id, now));
                    trigger_image_fetch(
                        shared,
                        ctx.plugin_id.map(|s| s.to_string()),
                        url.clone(),
                        key.to_string(),
                        ui.ctx().clone(),
                    );
                }
            }
        }
    }
}

fn render_list_item_text(
    ui: &mut egui::Ui,
    colors: &arclain_theme::ThemeColors,
    title: &str,
    subtitle: &Option<String>,
) {
    ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
        // Leave space for badge/icon on the right.
        ui.set_max_width(ui.available_width() - 80.0);

        ui.add(
            egui::Label::new(egui::RichText::new(title).strong().color(colors.on_surface))
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
}

fn render_list_item_meta(
    ui: &mut egui::Ui,
    colors: &arclain_theme::ThemeColors,
    badge: &Option<String>,
    warning_icon: &Option<WarningIcon>,
) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
