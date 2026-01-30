//! Main renderer for plugin UI elements

use super::context::{RenderContext, UiEventHandler};
use super::{image, layout, widgets};
use crate::shared::components::carousel::{Carousel, CarouselEvent};
use crate::shared::{theme::ThemeColors, SharedState};
use arclain_data::ContentCache;
use arclain_plugins::types::PluginUiElement;
use eframe::egui;
use std::sync::Arc;

/// Render a plugin UI element and its children
pub fn render_ui_element<'a, H: UiEventHandler + ?Sized>(
    ui: &mut egui::Ui,
    element: &PluginUiElement,
    event_callback: &'a mut H,
    colors: &'a ThemeColors,
    content_cache: Option<&'a Arc<ContentCache>>,
    shared_state: Option<&'a SharedState>,
    plugin_id: Option<&'a str>,
) {
    let mut ctx = RenderContext {
        event_callback,
        colors,
        content_cache,
        shared_state,
        plugin_id,
    };
    render_recursive(ui, element, &mut ctx);
}

/// Helper for recursive rendering
fn render_recursive<H: UiEventHandler + ?Sized>(
    ui: &mut egui::Ui,
    element: &PluginUiElement,
    ctx: &mut RenderContext<'_, H>,
) {
    match element {
        PluginUiElement::Column { children, spacing } => {
            layout::render_column(ui, ctx, children, *spacing, render_recursive);
        }
        PluginUiElement::Row { children, spacing } => {
            layout::render_row(ui, ctx, children, *spacing, render_recursive);
        }
        PluginUiElement::Grid { columns, children } => {
            layout::render_grid(ui, ctx, *columns, children, render_recursive);
        }
        PluginUiElement::ListContainer {
            id: _,
            items,
            max_height,
            empty_message,
        } => {
            layout::render_list_container(
                ui,
                ctx,
                items,
                *max_height,
                empty_message,
                render_recursive,
            );
        }
        PluginUiElement::Separator => {
            layout::render_separator(ui);
        }
        PluginUiElement::Space { size } => {
            layout::render_space(ui, *size);
        }
        PluginUiElement::Label { text, bold, size } => {
            widgets::render_label(ui, ctx, text, *bold, *size);
        }
        PluginUiElement::SectionHeader {
            title,
            level,
            description,
        } => {
            widgets::render_section_header(ui, ctx, title, *level, description);
        }
        PluginUiElement::Button { id, label, action } => {
            widgets::render_button(ui, ctx, id, label, action);
        }
        PluginUiElement::TextInput {
            id,
            label,
            value,
            placeholder,
        } => {
            widgets::render_text_input(ui, ctx, id, label, value, placeholder);
        }
        PluginUiElement::Checkbox { id, label, checked } => {
            widgets::render_checkbox(ui, ctx, id, label, *checked);
        }
        PluginUiElement::RadioGroup {
            id,
            label,
            options,
            selected,
        } => {
            widgets::render_radio_group(ui, ctx, id, label, options, selected);
        }
        PluginUiElement::Slider {
            id,
            label,
            value,
            min,
            max,
            step,
        } => {
            widgets::render_slider(
                ui,
                ctx,
                id,
                label,
                *value as f64,
                *min as f64,
                *max as f64,
                step.map(|s| s as f64),
            );
        }
        PluginUiElement::Dropdown {
            id,
            label,
            options,
            selected,
        } => {
            widgets::render_dropdown(ui, ctx, id, label, options, selected);
        }
        PluginUiElement::Tabs { id, tabs, selected } => {
            widgets::render_tabs(ui, ctx, id, tabs, selected);
        }
        PluginUiElement::Warning { icon, message } => {
            widgets::render_warning(ui, ctx, icon, message);
        }
        PluginUiElement::Loading { message } => {
            widgets::render_loading(ui, ctx, message);
        }
        PluginUiElement::TagChips { tags, max_display } => {
            widgets::render_tag_chips(ui, ctx, tags, *max_display);
        }
        PluginUiElement::Toolbar { buttons } => {
            widgets::render_toolbar(ui, ctx, buttons);
        }
        PluginUiElement::Image {
            cache_key,
            url,
            max_height,
        } => {
            image::render_image(ui, ctx, cache_key, url, *max_height);
        }
        PluginUiElement::ListItem {
            id,
            title,
            subtitle,
            badge,
            image_key,
            selected,
            warning_icon,
        } => {
            widgets::render_list_item(
                ui,
                ctx,
                id,
                title,
                subtitle,
                badge,
                image_key,
                *selected,
                warning_icon,
            );
        }
        PluginUiElement::Carousel {
            id,
            images,
            current_index,
            max_height,
            thumbnail_height,
            enable_lightbox,
        } => {
            let mut carousel = Carousel::new(id, images, *current_index)
                .main_height(max_height.unwrap_or(300.0))
                .thumbnail_height(thumbnail_height.unwrap_or(60.0))
                .enable_lightbox(*enable_lightbox)
                .colors(ctx.colors);

            if let Some(cache) = ctx.content_cache {
                carousel = carousel.content_cache(cache);
            }

            if let Some(event) = carousel.show(ui) {
                match event {
                    CarouselEvent::Previous => {
                        (ctx.event_callback)(&format!("{}_prev", id), None)
                    }
                    CarouselEvent::Next => (ctx.event_callback)(&format!("{}_next", id), None),
                    CarouselEvent::Select(idx) => {
                        (ctx.event_callback)(&format!("{}_select_{}", id, idx), None)
                    }
                    CarouselEvent::OpenLightbox => {
                        (ctx.event_callback)(&format!("{}_open_lightbox", id), None)
                    }
                }
            }
        }
        PluginUiElement::KeyValueList { items, columns } => {
            widgets::render_key_value_list(ui, ctx, items, *columns);
        }
        PluginUiElement::MetadataGrid { items, columns } => {
            widgets::render_metadata_grid(ui, ctx, items, *columns);
        }
    }
}

/// Determine if multiple elements should be rendered (a helper for `render_ui_elements`)
pub fn render_ui_elements<'a, H: UiEventHandler + ?Sized>(
    ui: &mut egui::Ui,
    elements: &[PluginUiElement],
    event_callback: &'a mut H,
    colors: &'a ThemeColors,
    content_cache: Option<&'a Arc<ContentCache>>,
    shared_state: Option<&'a SharedState>,
    plugin_id: Option<&'a str>,
) {
    let mut ctx = RenderContext {
        event_callback,
        colors,
        content_cache,
        shared_state,
        plugin_id,
    };
    for element in elements {
        render_recursive(ui, element, &mut ctx);
    }
}
