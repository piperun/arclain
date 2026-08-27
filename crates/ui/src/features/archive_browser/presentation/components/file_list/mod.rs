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

use crate::core::tabs::view_state::RevisionedSelection;
use crate::shared::theme::AppTheme;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;
use egui_extras::{Column, TableBuilder};

fn archive_action(
    entry: &FileEntry,
    create: impl FnOnce(String) -> FileListAction,
) -> FileListAction {
    create(entry.archive_path.clone())
}

pub(super) fn visible_drag_payload(
    entries: &[FileEntry],
    order: &[usize],
    selection: &RevisionedSelection,
    dragged: &FileEntry,
) -> Vec<String> {
    if !selection.contains(&dragged.archive_path) {
        return vec![dragged.archive_path.clone()];
    }

    order
        .iter()
        .map(|index| &entries[*index])
        .filter(|entry| selection.contains(&entry.archive_path))
        .map(|entry| entry.archive_path.clone())
        .collect()
}

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
            egui::Label::new(egui::RichText::new(text).size(14.0).color(color)).selectable(false),
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
    prev_selected: bool,
    next_selected: bool,
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

    let stroke_color = theme.colors.selection.linear_multiply(0.35);
    let stroke = egui::Stroke::new(1.0_f32, stroke_color);

    if !prev_selected {
        let y = fill_rect.min.y + 0.5;
        painter.line_segment(
            [
                egui::pos2(fill_rect.min.x, y),
                egui::pos2(fill_rect.max.x, y),
            ],
            stroke,
        );
    }
    if !next_selected {
        let y = fill_rect.max.y - 0.5;
        painter.line_segment(
            [
                egui::pos2(fill_rect.min.x, y),
                egui::pos2(fill_rect.max.x, y),
            ],
            stroke,
        );
    }
}

/// Build the row right-click context menu. Returns the action the
/// user picked, if any.
fn row_context_menu(
    row_response: &egui::Response,
    theme: &AppTheme,
    entry: &FileEntry,
) -> Option<FileListAction> {
    let mut picked: Option<FileListAction> = None;

    let menu_btn = |ui: &mut egui::Ui, label: &str| -> egui::Response {
        ui.add(TextButton::new(label, ButtonSize::Medium).with_theme_colors(&theme.colors))
    };

    row_response.context_menu(|ui| {
        if menu_btn(ui, "📂  Open").clicked() {
            picked = Some(if entry.is_folder {
                FileListAction::Navigate(entry.path.clone())
            } else {
                archive_action(entry, FileListAction::Open)
            });
            ui.close();
        }
        ui.separator();
        if menu_btn(ui, "📦  Extract").clicked() {
            picked = Some(archive_action(entry, FileListAction::Extract));
            ui.close();
        }
        if menu_btn(ui, "📁  Extract To...").clicked() {
            picked = Some(archive_action(entry, FileListAction::ExtractTo));
            ui.close();
        }
        ui.separator();
        if menu_btn(ui, "📋  Copy Path").clicked() {
            picked = Some(archive_action(entry, FileListAction::CopyPath));
            ui.close();
        }
        ui.separator();
        if menu_btn(ui, "ℹ️  Properties").clicked() {
            picked = Some(archive_action(entry, FileListAction::ShowProperties));
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(
                !entry.is_folder,
                TextButton::new("✏️  Edit", ButtonSize::Medium).with_theme_colors(&theme.colors),
            )
            .clicked()
        {
            picked = Some(archive_action(entry, FileListAction::Edit));
            ui.close();
        }
        if menu_btn(ui, "🗑️  Delete").clicked() {
            picked = Some(archive_action(entry, FileListAction::Delete));
            ui.close();
        }
    });

    picked
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
        if size {
            remaining -= COL_SIZE_W;
        }

        let type_col = remaining >= COL_TYPE_W;
        if type_col {
            remaining -= COL_TYPE_W;
        }

        let modified = remaining >= COL_MODIFIED_W;
        if modified {
            remaining -= COL_MODIFIED_W;
        }

        let compressed = remaining >= COL_COMPRESSED_W;
        if compressed {
            remaining -= COL_COMPRESSED_W;
        }

        let ratio = remaining >= COL_RATIO_W;
        if ratio {
            remaining -= COL_RATIO_W;
        }

        let crc = remaining >= COL_CRC_W;
        if crc {
            remaining -= COL_CRC_W;
        }

        let encrypted = remaining >= COL_ENCRYPTED_W;

        Self {
            size,
            type_col,
            modified,
            compressed,
            ratio,
            crc,
            encrypted,
        }
    }
}

pub fn render_list_view(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entries: &[FileEntry],
    order: &[usize],
    visible_selected_count: usize,
    selection: &mut RevisionedSelection,
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
            if vis.type_col {
                table = table.column(Column::exact(COL_TYPE_W));
            }
            if vis.size {
                table = table.column(Column::exact(COL_SIZE_W));
            }
            if vis.compressed {
                table = table.column(Column::exact(COL_COMPRESSED_W));
            }
            if vis.ratio {
                table = table.column(Column::exact(COL_RATIO_W));
            }
            if vis.modified {
                table = table.column(Column::exact(COL_MODIFIED_W));
            }
            if vis.crc {
                table = table.column(Column::exact(COL_CRC_W));
            }
            if vis.encrypted {
                table = table.column(Column::exact(COL_ENCRYPTED_W));
            }
            table
                .column(Column::exact(COL_ACTIONS_W))
                .header(28.0, |mut header| {
                    // Select all checkbox
                    header.col(|ui| {
                        let all_selected =
                            !order.is_empty() && visible_selected_count == order.len();
                        let some_selected = visible_selected_count > 0;
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
                    add_sort_header_col(
                        &mut header,
                        theme,
                        "Type",
                        SortColumn::Type,
                        sort,
                        vis.type_col,
                    );
                    add_sort_header_col(
                        &mut header,
                        theme,
                        "Size",
                        SortColumn::Size,
                        sort,
                        vis.size,
                    );
                    add_sort_header_col(
                        &mut header,
                        theme,
                        "Compressed",
                        SortColumn::Compressed,
                        sort,
                        vis.compressed,
                    );
                    add_sort_header_col(
                        &mut header,
                        theme,
                        "Ratio",
                        SortColumn::Ratio,
                        sort,
                        vis.ratio,
                    );
                    add_sort_header_col(
                        &mut header,
                        theme,
                        "Modified",
                        SortColumn::Modified,
                        sort,
                        vis.modified,
                    );
                    add_sort_header_col(
                        &mut header,
                        theme,
                        "CRC-32",
                        SortColumn::Crc32,
                        sort,
                        vis.crc,
                    );
                    add_sort_header_col(
                        &mut header,
                        theme,
                        "Encrypted",
                        SortColumn::Encrypted,
                        sort,
                        vis.encrypted,
                    );
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
                .body(|body| {
                    // Apply select-all toggle if requested
                    if let Some(v) = apply_select_all.take() {
                        if v {
                            for index in order {
                                selection.insert(entries[*index].archive_path.clone());
                            }
                        } else {
                            for index in order {
                                selection.remove(&entries[*index].archive_path);
                            }
                        }
                    }

                    // Virtualized row rendering: body.rows() only invokes
                    // the closure for rows currently in the scroll viewport
                    // (typically 20-40 rows on screen), not all `entries.len()`
                    // rows. Critical for large archives — 1000s of entries
                    // were O(n) per frame before this. Cursor-move repaints
                    // now scale with visible rows instead of total entries.
                    body.rows(32.0, order.len(), |mut row| {
                        let row_index = row.index();
                        let entry = &entries[order[row_index]];
                        let entry_name = entry.name.clone();
                        let entry_path = entry.path.clone();
                        let archive_path = entry.archive_path.clone();
                        let is_folder = entry.is_folder;

                        {
                            let mut checkbox_clicked = false;
                            let mut action_clicked = false;
                            let text_color = theme.colors.on_surface;
                            let muted_color = theme.colors.on_surface_variant;

                            row.col(|ui| {
                                // egui::checkbox needs a `&mut bool`; selection
                                // lives in a HashSet keyed by stable archive
                                // path, so we use a local bool synced both ways.
                                let mut checked = selection.contains(&archive_path);
                                let response = ui.checkbox(&mut checked, "");
                                if response.clicked() {
                                    if checked {
                                        selection.insert(archive_path.clone());
                                    } else {
                                        selection.remove(&archive_path);
                                    }
                                }
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
                                            Some(archive_action(entry, FileListAction::Edit));
                                    }

                                    let del_clicked = ui
                                        .add_sized(egui::vec2(28.0, 22.0), egui::Button::new("🗑"))
                                        .on_hover_text("Delete")
                                        .clicked();
                                    if del_clicked {
                                        action_clicked = true;
                                        pending_row_action =
                                            Some(archive_action(entry, FileListAction::Delete));
                                    }
                                });
                            });

                            let row_response = row.response();

                            let is_selected = selection.contains(&archive_path);
                            if is_selected {
                                let prev_selected = row_index
                                    .checked_sub(1)
                                    .map(|previous| {
                                        selection.contains(&entries[order[previous]].archive_path)
                                    })
                                    .unwrap_or(false);
                                let next_selected = order
                                    .get(row_index + 1)
                                    .map(|next| selection.contains(&entries[*next].archive_path))
                                    .unwrap_or(false);
                                paint_row_selection(
                                    &row_response,
                                    list_clip_rect,
                                    theme,
                                    prev_selected,
                                    next_selected,
                                );
                            }
                            if row_response.hovered() {
                                row_response
                                    .ctx
                                    .set_cursor_icon(egui::CursorIcon::PointingHand);
                            }

                            if let Some(menu_action) = row_context_menu(&row_response, theme, entry)
                            {
                                action = Some(menu_action);
                            }

                            // Drag started - collect selected files/folders for drag-out
                            if row_response.drag_started() {
                                // Show grab cursor for visual feedback
                                row_response.ctx.set_cursor_icon(egui::CursorIcon::Grabbing);

                                // If the dragged row isn't selected, just drag that one.
                                // Build the visible selected payload only for the frame where a
                                // drag actually starts; settled frames perform no full scan.
                                let files_to_drag =
                                    visible_drag_payload(entries, order, selection, entry);
                                if !files_to_drag.is_empty() {
                                    action = Some(FileListAction::DragStarted(files_to_drag));
                                }
                            } else if row_response.double_clicked() {
                                if is_folder {
                                    action = Some(FileListAction::Navigate(entry_path.clone()));
                                } else {
                                    action = Some(archive_action(entry, FileListAction::Open));
                                }
                            } else if !checkbox_clicked && !action_clicked && row_response.clicked()
                            {
                                // Toggle selection in the HashSet
                                if selection.contains(&archive_path) {
                                    selection.remove(&archive_path);
                                } else {
                                    selection.insert(archive_path.clone());
                                }
                            }

                            if action.is_none() {
                                if let Some(a) = pending_row_action.take() {
                                    action = Some(a);
                                }
                            }
                        }
                    });
                });
        });

    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_delete_action_uses_stable_archive_root_path() {
        let entry = FileEntry {
            name: "same.txt".to_string(),
            path: "same.txt".to_string(),
            archive_path: "A/same.txt".to_string(),
            size: "1 B".to_string(),
            compressed: "1 B".to_string(),
            ratio: "0%".to_string(),
            modified: String::new(),
            crc32: String::new(),
            encrypted: false,
            is_folder: false,
        };
        let action = archive_action(&entry, FileListAction::Delete);

        assert!(matches!(
            &action,
            FileListAction::Delete(path) if path == "A/same.txt"
        ));
    }
}
