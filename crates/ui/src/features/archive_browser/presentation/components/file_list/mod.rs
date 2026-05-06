//! File list component for archive browsing
//!
//! This module provides list and grid views for displaying archive contents.
//! Split into submodules for maintainability:
//! - `types` - Data structures and enums
//! - `breadcrumb` - Breadcrumb navigation
//! - `grid` - Grid view rendering

mod breadcrumb;
mod grid;
mod types;

// Re-export public API
pub use breadcrumb::render_breadcrumb;
pub use grid::render_grid_view;
pub use types::{
    parse_ratio_pct, parse_size_to_bytes, FileEntry, FileListAction, SortColumn, SortState,
};

use crate::shared::theme::AppTheme;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;
use egui_extras::{Column, TableBuilder};

// ================= List View (sortable + select-all) =================

fn header_sort_label(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    label: &str,
    current: SortColumn,
    this_col: SortColumn,
    ascending: bool,
) -> bool {
    let mut text = label.to_string();
    if current == this_col {
        text.push(' ');
        text.push(if ascending { '▲' } else { '▼' });
    }
    let resp = ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(12.0)
                .strong()
                .color(theme.colors.on_surface_variant),
        )
        .selectable(false)
        .sense(egui::Sense::click()),
    );
    resp.clicked()
}

/// Apply a header click: flip ascending if the same column is
/// already active, otherwise switch to that column and reset to
/// ascending order.
fn apply_sort_click(sort: &mut SortState, this_col: SortColumn) {
    if sort.column == this_col {
        sort.ascending = !sort.ascending;
    } else {
        sort.column = this_col;
        sort.ascending = true;
    }
}

/// Add an optional sortable header column. No-op if `visible` is
/// false. Replaces 7 near-identical `if vis.X { header.col(|ui| ...) }`
/// arms with one call each.
fn add_sort_header_col(
    header: &mut egui_extras::TableRow<'_, '_>,
    theme: &AppTheme,
    label: &str,
    this_col: SortColumn,
    sort: &mut SortState,
    visible: bool,
) {
    if !visible {
        return;
    }
    header.col(|ui| {
        if header_sort_label(ui, theme, label, sort.column, this_col, sort.ascending) {
            apply_sort_click(sort, this_col);
        }
    });
}

/// Add a plain text body cell. Most optional columns are just
/// `Label::new(text).size(14.0).color(c).selectable(false)`.
fn add_text_col(
    row: &mut egui_extras::TableRow<'_, '_>,
    visible: bool,
    text: &str,
    color: egui::Color32,
) {
    if !visible {
        return;
    }
    row.col(|ui| {
        ui.add(
            egui::Label::new(egui::RichText::new(text).size(14.0).color(color))
                .selectable(false),
        );
    });
}

/// Paint the selection highlight for a row: a translucent fill plus
/// a 1px border on the top edge if the previous row isn't selected
/// and the bottom edge if the next row isn't, so a contiguous block
/// of selected rows draws as a single rounded slab.
fn paint_row_selection(
    row_response: &egui::Response,
    list_clip_rect: egui::Rect,
    theme: &AppTheme,
    row_index: usize,
    selection_flags: &[bool],
) {
    let painter = egui::Painter::new(
        row_response.ctx.clone(),
        row_response.layer_id,
        list_clip_rect,
    );

    let mut fill_rect = row_response.rect.shrink2(egui::vec2(2.0, 0.0));
    fill_rect.min.y -= 0.5;
    fill_rect.max.y += 0.5;

    let fill_color = theme.colors.selection.linear_multiply(0.14);
    painter.rect_filled(fill_rect, 0.0, fill_color);

    let prev_selected = row_index > 0 && selection_flags[row_index - 1];
    let next_selected =
        row_index + 1 < selection_flags.len() && selection_flags[row_index + 1];
    let stroke_color = theme.colors.selection.linear_multiply(0.35);
    let stroke = egui::Stroke::new(1.0, stroke_color);

    if !prev_selected {
        let y = fill_rect.min.y + 0.5;
        painter.line_segment(
            [egui::pos2(fill_rect.min.x, y), egui::pos2(fill_rect.max.x, y)],
            stroke,
        );
    }
    if !next_selected {
        let y = fill_rect.max.y - 0.5;
        painter.line_segment(
            [egui::pos2(fill_rect.min.x, y), egui::pos2(fill_rect.max.x, y)],
            stroke,
        );
    }
}

/// Build the row right-click context menu. Returns the action the
/// user picked, if any.
fn row_context_menu(
    row_response: &egui::Response,
    theme: &AppTheme,
    entry_path: &str,
    is_folder: bool,
) -> Option<FileListAction> {
    let mut picked: Option<FileListAction> = None;

    let menu_btn = |ui: &mut egui::Ui, label: &str| -> egui::Response {
        ui.add(TextButton::new(label, ButtonSize::Medium).with_theme_colors(&theme.colors))
    };

    row_response.context_menu(|ui| {
        if menu_btn(ui, "📂  Open").clicked() {
            picked = Some(if is_folder {
                FileListAction::Navigate(entry_path.to_string())
            } else {
                FileListAction::Open(entry_path.to_string())
            });
            ui.close();
        }
        ui.separator();
        if menu_btn(ui, "📦  Extract").clicked() {
            picked = Some(FileListAction::Extract(entry_path.to_string()));
            ui.close();
        }
        if menu_btn(ui, "📁  Extract To...").clicked() {
            picked = Some(FileListAction::ExtractTo(entry_path.to_string()));
            ui.close();
        }
        ui.separator();
        if menu_btn(ui, "📋  Copy Path").clicked() {
            picked = Some(FileListAction::CopyPath(entry_path.to_string()));
            ui.close();
        }
        ui.separator();
        if menu_btn(ui, "ℹ️  Properties").clicked() {
            picked = Some(FileListAction::ShowProperties(entry_path.to_string()));
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(
                !is_folder,
                TextButton::new("✏️  Edit", ButtonSize::Medium).with_theme_colors(&theme.colors),
            )
            .clicked()
        {
            picked = Some(FileListAction::Edit(entry_path.to_string()));
            ui.close();
        }
        if menu_btn(ui, "🗑️  Delete").clicked() {
            picked = Some(FileListAction::Delete(entry_path.to_string()));
            ui.close();
        }
    });

    picked
}

fn sort_entries(entries: &mut [FileEntry], sort: &SortState) {
    // Use sort_by_cached_key where the key involves allocation (Name, Type, Crc32)
    // to avoid repeated allocations per comparison. Other columns use cheap keys.
    match sort.column {
        SortColumn::Name => {
            entries.sort_by_cached_key(|e| e.name.to_lowercase());
        }
        SortColumn::Type => {
            entries.sort_by_cached_key(|e| {
                if e.is_folder {
                    "directory".to_string()
                } else {
                    e.name.split('.').last().unwrap_or("file").to_lowercase()
                }
            });
        }
        SortColumn::Size => {
            entries.sort_by_cached_key(|e| parse_size_to_bytes(&e.size));
        }
        SortColumn::Compressed => {
            entries.sort_by_cached_key(|e| parse_size_to_bytes(&e.compressed));
        }
        SortColumn::Ratio => {
            entries.sort_by_cached_key(|e| parse_ratio_pct(&e.ratio));
        }
        SortColumn::Modified => {
            entries.sort_by_cached_key(|e| e.modified.clone());
        }
        SortColumn::Crc32 => {
            entries.sort_by_cached_key(|e| e.crc32.to_uppercase());
        }
        SortColumn::Encrypted => {
            entries.sort_by_cached_key(|e| e.encrypted as u8);
        }
    }
    if !sort.ascending {
        entries.reverse();
    }
}

/// Which optional columns fit at the current viewport width.
///
/// Checkbox, Name, and Actions are always shown. Everything else is hidden
/// in priority order when the window is too narrow to fit it, so the final
/// "Actions" column never clips off-screen.
#[derive(Copy, Clone, Debug)]
struct ColumnVisibility {
    size: bool,
    type_col: bool,
    modified: bool,
    compressed: bool,
    ratio: bool,
    crc: bool,
    encrypted: bool,
}

const COL_CHECKBOX_W: f32 = 34.0;
const COL_NAME_MIN_W: f32 = 220.0;
const COL_TYPE_W: f32 = 80.0;
const COL_SIZE_W: f32 = 110.0;
const COL_COMPRESSED_W: f32 = 110.0;
const COL_RATIO_W: f32 = 76.0;
const COL_MODIFIED_W: f32 = 140.0;
const COL_CRC_W: f32 = 120.0;
const COL_ENCRYPTED_W: f32 = 80.0;
const COL_ACTIONS_W: f32 = 84.0;

impl ColumnVisibility {
    /// Decide which optional columns to show given the table's available width.
    /// Priority (first shown, last hidden): Size, Type, Modified, Compressed, Ratio, CRC-32, Encrypted.
    fn for_width(available: f32) -> Self {
        // Reserve space for always-on columns; Name consumes the rest via Column::remainder.
        let must_have = COL_CHECKBOX_W + COL_NAME_MIN_W + COL_ACTIONS_W;
        let mut remaining = (available - must_have).max(0.0);

        let size = remaining >= COL_SIZE_W;
        if size { remaining -= COL_SIZE_W; }

        let type_col = remaining >= COL_TYPE_W;
        if type_col { remaining -= COL_TYPE_W; }

        let modified = remaining >= COL_MODIFIED_W;
        if modified { remaining -= COL_MODIFIED_W; }

        let compressed = remaining >= COL_COMPRESSED_W;
        if compressed { remaining -= COL_COMPRESSED_W; }

        let ratio = remaining >= COL_RATIO_W;
        if ratio { remaining -= COL_RATIO_W; }

        let crc = remaining >= COL_CRC_W;
        if crc { remaining -= COL_CRC_W; }

        let encrypted = remaining >= COL_ENCRYPTED_W;

        Self { size, type_col, modified, compressed, ratio, crc, encrypted }
    }
}

pub fn render_list_view(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entries: &mut [FileEntry],
    columns_locked: bool,
    sort: &mut SortState,
) -> Option<FileListAction> {
    let mut action: Option<FileListAction> = None;

    // Header-driven actions to apply before drawing body
    let mut apply_select_all: Option<bool> = None;

    // Clip rectangle for row decorations
    let list_clip_rect = ui.clip_rect();

    egui::Frame::NONE
        .fill(egui::Color32::TRANSPARENT)
        .inner_margin(egui::Margin {
            left: 16,
            right: 16,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            let table_id = if columns_locked {
                egui::Id::new("file_list_table_locked")
            } else {
                egui::Id::new("file_list_table_resizable")
            };

            let vis = ColumnVisibility::for_width(ui.available_width());

            let mut table = TableBuilder::new(ui)
                .id_salt(table_id)
                .striped(false)
                .resizable(!columns_locked)
                .sense(egui::Sense::click_and_drag())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(COL_CHECKBOX_W))
                .column(Column::remainder().at_least(COL_NAME_MIN_W));
            if vis.type_col { table = table.column(Column::exact(COL_TYPE_W)); }
            if vis.size { table = table.column(Column::exact(COL_SIZE_W)); }
            if vis.compressed { table = table.column(Column::exact(COL_COMPRESSED_W)); }
            if vis.ratio { table = table.column(Column::exact(COL_RATIO_W)); }
            if vis.modified { table = table.column(Column::exact(COL_MODIFIED_W)); }
            if vis.crc { table = table.column(Column::exact(COL_CRC_W)); }
            if vis.encrypted { table = table.column(Column::exact(COL_ENCRYPTED_W)); }
            table
                .column(Column::exact(COL_ACTIONS_W))
                .header(28.0, |mut header| {
                    // Select all checkbox
                    header.col(|ui| {
                        let all_selected =
                            !entries.is_empty() && entries.iter().all(|e| e.selected);
                        let some_selected = entries.iter().any(|e| e.selected);
                        let mut header_check = all_selected;
                        let resp = ui.checkbox(&mut header_check, "");
                        if resp.clicked() {
                            apply_select_all = Some(header_check);
                        } else if some_selected && !all_selected && resp.hovered() {
                            resp.on_hover_text("Some rows selected — click to toggle select all");
                        }
                    });

                    header.col(|ui| {
                        if header_sort_label(
                            ui,
                            theme,
                            "Name",
                            sort.column,
                            SortColumn::Name,
                            sort.ascending,
                        ) {
                            apply_sort_click(sort, SortColumn::Name);
                        }
                    });
                    add_sort_header_col(&mut header, theme, "Type", SortColumn::Type, sort, vis.type_col);
                    add_sort_header_col(&mut header, theme, "Size", SortColumn::Size, sort, vis.size);
                    add_sort_header_col(&mut header, theme, "Compressed", SortColumn::Compressed, sort, vis.compressed);
                    add_sort_header_col(&mut header, theme, "Ratio", SortColumn::Ratio, sort, vis.ratio);
                    add_sort_header_col(&mut header, theme, "Modified", SortColumn::Modified, sort, vis.modified);
                    add_sort_header_col(&mut header, theme, "CRC-32", SortColumn::Crc32, sort, vis.crc);
                    add_sort_header_col(&mut header, theme, "Encrypted", SortColumn::Encrypted, sort, vis.encrypted);
                    header.col(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("Actions")
                                    .size(12.0)
                                    .strong()
                                    .color(theme.colors.on_surface_variant),
                            )
                            .selectable(false),
                        );
                    });
                })
                .body(|mut body| {
                    // Apply select-all toggle if requested
                    if let Some(v) = apply_select_all.take() {
                        for e in entries.iter_mut() {
                            e.selected = v;
                        }
                    }

                    // Sort entries based on current sort state
                    sort_entries(entries, sort);

                    // Capture selection flags after sorting
                    let selection_flags: Vec<bool> = entries.iter().map(|e| e.selected).collect();

                    // Pre-collect selected file paths for drag-out (needed because we can't borrow entries during iter_mut)
                    // Note: Include both files AND folders for drag
                    let selected_files: Vec<String> = entries
                        .iter()
                        .filter(|e| e.selected)
                        .map(|e| e.path.clone())
                        .collect();

                    for (row_index, entry) in entries.iter_mut().enumerate() {
                        let entry_name = entry.name.clone();
                        let entry_path = entry.path.clone();
                        let is_folder = entry.is_folder;

                        body.row(32.0, |mut row| {
                            let mut checkbox_clicked = false;
                            let mut action_clicked = false;
                            let text_color = theme.colors.on_surface;
                            let muted_color = theme.colors.on_surface_variant;

                            row.col(|ui| {
                                let response = ui.checkbox(&mut entry.selected, "");
                                checkbox_clicked = response.clicked();
                            });

                            row.col(|ui| {
                                let icon = if is_folder {
                                    egui_phosphor::regular::FOLDER
                                } else {
                                    egui_phosphor::regular::FILE
                                };
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!("{icon} {entry_name}"))
                                            .size(14.0)
                                            .color(text_color),
                                    )
                                    .selectable(false),
                                );
                            });

                            let type_str = if is_folder {
                                "Folder".to_string()
                            } else {
                                entry_name
                                    .split('.')
                                    .last()
                                    .unwrap_or("File")
                                    .to_uppercase()
                            };
                            add_text_col(&mut row, vis.type_col, &type_str, muted_color);
                            add_text_col(&mut row, vis.size, &entry.size, text_color);
                            add_text_col(&mut row, vis.compressed, &entry.compressed, text_color);
                            add_text_col(&mut row, vis.ratio, &entry.ratio, text_color);
                            add_text_col(&mut row, vis.modified, &entry.modified, muted_color);
                            add_text_col(&mut row, vis.crc, &entry.crc32, text_color);

                            if vis.encrypted {
                                row.col(|ui| {
                                    if !entry.is_folder {
                                        let (text, color) = if entry.encrypted {
                                            ("Yes", theme.colors.on_surface)
                                        } else {
                                            ("No", theme.colors.on_surface_variant)
                                        };
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(text).size(14.0).color(color),
                                            )
                                            .selectable(false),
                                        );
                                    }
                                });
                            }

                            // Actions column
                            let mut pending_row_action: Option<FileListAction> = None;
                            row.col(|ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;

                                    let can_edit = !is_folder;
                                    let hover_text = if is_folder {
                                        "Cannot edit folders"
                                    } else {
                                        "Edit file (rename and/or edit content for text files)"
                                    };

                                    let edit_clicked = ui
                                        .add_enabled(
                                            can_edit,
                                            egui::Button::new("✏").min_size(egui::vec2(28.0, 22.0)),
                                        )
                                        .on_hover_text(hover_text)
                                        .clicked();
                                    if edit_clicked {
                                        action_clicked = true;
                                        pending_row_action =
                                            Some(FileListAction::Edit(entry_path.clone()));
                                    }

                                    let del_clicked = ui
                                        .add_sized(egui::vec2(28.0, 22.0), egui::Button::new("🗑"))
                                        .on_hover_text("Delete")
                                        .clicked();
                                    if del_clicked {
                                        action_clicked = true;
                                        pending_row_action =
                                            Some(FileListAction::Delete(entry_path.clone()));
                                    }
                                });
                            });

                            let row_response = row.response();

                            if entry.selected {
                                paint_row_selection(
                                    &row_response,
                                    list_clip_rect,
                                    theme,
                                    row_index,
                                    &selection_flags,
                                );
                            }
                            if row_response.hovered() {
                                row_response
                                    .ctx
                                    .set_cursor_icon(egui::CursorIcon::PointingHand);
                            }

                            if let Some(menu_action) =
                                row_context_menu(&row_response, theme, &entry_path, is_folder)
                            {
                                action = Some(menu_action);
                            }

                            // Drag started - collect selected files/folders for drag-out
                            if row_response.drag_started() {
                                // Show grab cursor for visual feedback
                                row_response.ctx.set_cursor_icon(egui::CursorIcon::Grabbing);

                                // If the dragged row isn't selected, just drag that one
                                // Otherwise drag all selected entries (uses pre-collected list)
                                let files_to_drag: Vec<String> = if entry.selected {
                                    selected_files.clone()
                                } else {
                                    vec![entry_path.clone()]
                                };
                                if !files_to_drag.is_empty() {
                                    action = Some(FileListAction::DragStarted(files_to_drag));
                                }
                            } else if row_response.double_clicked() {
                                if is_folder {
                                    action = Some(FileListAction::Navigate(entry_path.clone()));
                                } else {
                                    action = Some(FileListAction::Open(entry_path.clone()));
                                }
                            } else if !checkbox_clicked && !action_clicked && row_response.clicked()
                            {
                                entry.selected = !entry.selected;
                            }

                            if action.is_none() {
                                if let Some(a) = pending_row_action.take() {
                                    action = Some(a);
                                }
                            }
                        });
                    }
                });
        });

    action
}
