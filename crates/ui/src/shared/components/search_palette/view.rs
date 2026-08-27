//! egui rendering + keyboard interaction for the unified search palette.
//!
//! The palette is a floating [`egui::Area`] anchored under the header
//! search box. The model ([`super::model`]) decides *what* matches; this
//! module decides *how* it looks and how the keyboard drives it.
//!
//! Keyboard note: [`handle_keys`] MUST run *before* the search `TextEdit`
//! renders. A focused single-line edit surrenders focus on Enter (which
//! would close the palette before activation is read) and moves the
//! caret on Up/Down. Consuming those keys first means the edit never
//! reacts to them.

use super::model::{match_range, SearchHit};
use crate::core::tabs::TabId;
use crate::shared::theme::AppTheme;
use eframe::egui;
use egui::text::{LayoutJob, TextFormat};

/// Cross-frame palette state, stored on `HeaderState`.
#[derive(Default)]
pub struct SearchPaletteState {
    /// Whether the dropdown is showing. Driven by search-box focus, but
    /// sticky enough that `handle_keys` can consume nav keys the frame
    /// before focus-derived close (Enter/Esc close it explicitly).
    pub open: bool,
    /// Flattened selection index across all result rows.
    pub selected: usize,
}

/// What activating a result row does. Dispatched by the app update loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchPaletteAction {
    /// Switch to an open tab.
    SwitchTab(TabId),
    /// Navigate the active archive's file list to this entry path.
    JumpToFile(String),
}

/// The action a given hit performs when activated.
pub fn action_for(hit: &SearchHit) -> SearchPaletteAction {
    match hit {
        SearchHit::Tab(t) => SearchPaletteAction::SwitchTab(t.id),
        SearchHit::File { path } => SearchPaletteAction::JumpToFile(path.clone()),
    }
}

/// Outcome of consuming the palette's navigation keys for this frame.
#[derive(Default)]
pub struct KeyIntent {
    /// Enter pressed — activate the selected row.
    pub activate: bool,
    /// Escape pressed — close the palette.
    pub dismiss: bool,
    /// Up/Down moved the selection — the dropdown should scroll the newly
    /// selected row into view this frame.
    pub navigated: bool,
}

/// Consume Up/Down/Enter/Escape and move `selected`. Call only when the
/// palette is open, and BEFORE the search `TextEdit` renders (see module
/// docs). `hits_len` bounds the wrap-around.
pub fn handle_keys(ui: &egui::Ui, hits_len: usize, selected: &mut usize) -> KeyIntent {
    let mut intent = KeyIntent::default();
    ui.input_mut(|i| {
        if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
            intent.dismiss = true;
        }
        if hits_len > 0 {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                *selected = (*selected + 1) % hits_len;
                intent.navigated = true;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                *selected = (*selected + hits_len - 1) % hits_len;
                intent.navigated = true;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                intent.activate = true;
            }
        }
    });
    intent
}

/// The palette's per-frame render data, grouped so [`render_area`] keeps a
/// small signature.
pub struct PaletteView<'a> {
    /// The search box rect; the dropdown anchors directly under it.
    pub anchor_rect: egui::Rect,
    pub query: &'a str,
    pub hits: &'a [SearchHit],
    /// Active archive's code; labels the file group ("Files in RJ…").
    pub active_code: &'a str,
    /// The caller just navigated with the arrows — scroll the selected row
    /// into view this frame.
    pub scroll_to_selected: bool,
}

/// Render the results dropdown as a floating Area anchored under the search
/// box. Returns `Some(action)` when a row is clicked. A *moving* pointer
/// over a row updates `selected`; a stationary pointer does not, so keyboard
/// nav isn't yanked back to wherever the mouse happens to rest.
pub fn render_area(
    ui: &egui::Ui,
    theme: &AppTheme,
    view: &PaletteView<'_>,
    selected: &mut usize,
) -> Option<SearchPaletteAction> {
    let PaletteView {
        anchor_rect,
        query,
        hits,
        active_code,
        scroll_to_selected,
    } = *view;
    // Keep the selection inside the current result set.
    if hits.is_empty() {
        *selected = 0;
    } else if *selected >= hits.len() {
        *selected = hits.len() - 1;
    }

    let tab_count = hits
        .iter()
        .filter(|h| matches!(h, SearchHit::Tab(_)))
        .count();
    let file_count = hits.len() - tab_count;
    let width = anchor_rect.width().max(360.0);

    let colors = &theme.colors;
    let selected_fill = egui::Color32::from_rgba_unmultiplied(
        colors.primary.r(),
        colors.primary.g(),
        colors.primary.b(),
        38,
    );

    let mut clicked: Option<usize> = None;
    let mut hovered: Option<usize> = None;

    egui::Area::new(egui::Id::new("search_palette"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(anchor_rect.left(), anchor_rect.bottom() + 6.0))
        .constrain(true)
        .show(ui.ctx(), |ui| {
            ui.set_max_width(width);
            egui::Frame::NONE
                .fill(colors.surface_variant)
                .stroke(egui::Stroke::new(1.0_f32, colors.outline))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(0, 8))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(120),
                })
                .show(ui, |ui| {
                    ui.set_width(width);
                    if hits.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(18.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "No tabs or files match \u{201c}{}\u{201d}",
                                    query.trim()
                                ))
                                .color(colors.on_surface_variant),
                            );
                            ui.add_space(18.0);
                        });
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .max_height(440.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (idx, hit) in hits.iter().enumerate() {
                                // Group headers at each kind transition.
                                if matches!(hit, SearchHit::Tab(_)) && idx == 0 {
                                    let label = if query.trim().is_empty() {
                                        "Open tabs \u{2014} jump to".to_string()
                                    } else {
                                        "Tabs".to_string()
                                    };
                                    group_label(ui, colors, &label, tab_count);
                                } else if matches!(hit, SearchHit::File { .. }) && idx == tab_count
                                {
                                    let where_ = if active_code.is_empty() {
                                        "active archive".to_string()
                                    } else {
                                        active_code.to_string()
                                    };
                                    group_label(
                                        ui,
                                        colors,
                                        &format!("Files in {where_}"),
                                        file_count,
                                    );
                                }

                                let is_sel = idx == *selected;
                                let fill = if is_sel {
                                    selected_fill
                                } else {
                                    egui::Color32::TRANSPARENT
                                };
                                let resp = render_row(ui, theme, hit, query, is_sel, fill, width);
                                if is_sel && scroll_to_selected {
                                    resp.scroll_to_me(Some(egui::Align::Center));
                                }
                                if resp.clicked() {
                                    clicked = Some(idx);
                                }
                                if resp.hovered() {
                                    hovered = Some(idx);
                                }
                            }
                        });
                });
        });

    // Only let hover drive the selection when the pointer actually moved
    // this frame. A stationary mouse resting over the list must not keep
    // overriding keyboard nav (the classic combo-box keyboard/mouse fight).
    let pointer_moved = ui.input(|i| {
        i.events
            .iter()
            .any(|e| matches!(e, egui::Event::PointerMoved(_)))
    });
    if pointer_moved {
        if let Some(h) = hovered {
            *selected = h;
        }
    }
    clicked.map(|i| action_for(&hits[i]))
}

fn group_label(ui: &mut egui::Ui, colors: &arclain_theme::ThemeColors, text: &str, count: usize) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(13.0);
        ui.label(
            egui::RichText::new(text.to_uppercase())
                .size(10.0)
                .color(colors.on_surface_variant),
        );
        ui.label(
            egui::RichText::new(count.to_string())
                .size(10.0)
                .color(colors.primary),
        );
    });
}

fn render_row(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    hit: &SearchHit,
    query: &str,
    is_sel: bool,
    fill: egui::Color32,
    width: f32,
) -> egui::Response {
    let colors = &theme.colors;
    let inner = egui::Frame::NONE
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(width - 24.0);
            ui.horizontal(|ui| {
                match hit {
                    SearchHit::Tab(t) => {
                        ui.label(
                            egui::RichText::new(egui_phosphor::regular::BROWSERS)
                                .color(colors.primary),
                        );
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            // primary: title + code
                            let mut primary = LayoutJob::default();
                            append_hl(
                                &mut primary,
                                &t.title,
                                query,
                                colors.on_surface,
                                colors.primary,
                                13.0,
                            );
                            append_plain(&mut primary, "  ", colors.on_surface_variant, 13.0);
                            append_hl(
                                &mut primary,
                                &t.code,
                                query,
                                colors.on_surface_variant,
                                colors.primary,
                                13.0,
                            );
                            ui.label(primary);

                            // secondary: file · N files · maker
                            let mut secondary = LayoutJob::default();
                            append_hl(
                                &mut secondary,
                                &t.file,
                                query,
                                colors.on_surface_variant,
                                colors.primary,
                                11.0,
                            );
                            append_plain(
                                &mut secondary,
                                &format!("  \u{00b7}  {} files", t.entry_count),
                                colors.on_surface_variant,
                                11.0,
                            );
                            if !t.maker.is_empty() {
                                append_plain(
                                    &mut secondary,
                                    "  \u{00b7}  ",
                                    colors.on_surface_variant,
                                    11.0,
                                );
                                append_hl(
                                    &mut secondary,
                                    &t.maker,
                                    query,
                                    colors.on_surface_variant,
                                    colors.primary,
                                    11.0,
                                );
                            }
                            ui.label(secondary);
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if is_sel {
                                ui.label(
                                    egui::RichText::new(egui_phosphor::regular::ARROW_RIGHT)
                                        .size(10.0)
                                        .color(colors.primary),
                                );
                            }
                            if t.active {
                                ui.label(
                                    egui::RichText::new("active")
                                        .size(9.0)
                                        .color(colors.primary),
                                );
                            }
                        });
                    }
                    SearchHit::File { path } => {
                        ui.label(
                            egui::RichText::new(egui_phosphor::regular::FILE)
                                .color(colors.on_surface_variant),
                        );
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            let mut primary = LayoutJob::default();
                            append_hl(
                                &mut primary,
                                path,
                                query,
                                colors.on_surface,
                                colors.primary,
                                13.0,
                            );
                            ui.label(primary);
                            if let Some(ext) = path
                                .rsplit('.')
                                .next()
                                .filter(|e| !e.contains('/') && *e != path.as_str())
                            {
                                ui.label(
                                    egui::RichText::new(ext.to_uppercase())
                                        .size(11.0)
                                        .color(colors.on_surface_variant),
                                );
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if is_sel {
                                ui.label(
                                    egui::RichText::new(egui_phosphor::regular::ARROW_RIGHT)
                                        .size(10.0)
                                        .color(colors.primary),
                                );
                            }
                        });
                    }
                }
            });
        });
    inner.response.interact(egui::Sense::click())
}

fn append_plain(job: &mut LayoutJob, text: &str, color: egui::Color32, size: f32) {
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: egui::FontId::proportional(size),
            color,
            ..Default::default()
        },
    );
}

/// Append `text`, coloring the first case-insensitive `query` match with
/// `hl` and the rest with `base`.
fn append_hl(
    job: &mut LayoutJob,
    text: &str,
    query: &str,
    base: egui::Color32,
    hl: egui::Color32,
    size: f32,
) {
    match match_range(text, query.trim()) {
        Some((s, e)) => {
            append_plain(job, &text[..s], base, size);
            append_plain(job, &text[s..e], hl, size);
            append_plain(job, &text[e..], base, size);
        }
        None => append_plain(job, text, base, size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::components::search_palette::model::TabSummary;

    #[test]
    fn action_for_maps_tab_to_switch_and_file_to_jump() {
        let t = TabSummary {
            id: TabId(3),
            code: "RJ1".into(),
            title: "T".into(),
            maker: "M".into(),
            file: "f.rar".into(),
            entry_count: 0,
            active: false,
        };
        assert_eq!(
            action_for(&SearchHit::Tab(t)),
            SearchPaletteAction::SwitchTab(TabId(3))
        );
        assert_eq!(
            action_for(&SearchHit::File {
                path: "dir/x.txt".into()
            }),
            SearchPaletteAction::JumpToFile("dir/x.txt".into())
        );
    }
}
