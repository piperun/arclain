//! Grid/card view for the archive file browser
//!
//! Renders files and folders as responsive vertical cards with file type icons,
//! hover-reveal actions, context menus, and selection support.

use super::types::{FileEntry, FileListAction};
use crate::core::tabs::view_state::RevisionedSelection;
use crate::shared::theme::AppTheme;
use arclain_widgets::{pixel_align, ButtonSize, TextButton};
use eframe::egui;

// ── Layout constants ────────────────────────────────────────────────

const CARD_WIDTH: f32 = 150.0;
const CARD_HEIGHT: f32 = 130.0;
const CARD_PADDING: f32 = 8.0;
const CARD_ROUNDING: f32 = 6.0;
const ICON_SIZE: f32 = 40.0;
const GRID_SPACING: f32 = 6.0;

/// Render files in a responsive grid of vertical cards
pub fn render_grid_view(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entries: &[FileEntry],
    order: &[usize],
    selection: &mut RevisionedSelection,
) -> Option<FileListAction> {
    let mut action: Option<FileListAction> = None;
    let available_width = ui.available_width() - 16.0; // account for frame margin
    let columns = ((available_width + GRID_SPACING) / (CARD_WIDTH + GRID_SPACING))
        .floor()
        .max(1.0) as usize;

    // Virtualized grid: ScrollArea::show_rows only invokes the row
    // closure for visually-visible rows. The previous implementation
    // iterated all entries.len() cards per frame (with an internal
    // `is_rect_visible` paint-cull that skipped painting off-screen
    // cards but still allocated their rects). show_rows skips the
    // allocation entirely for off-screen rows, dropping per-frame
    // work from O(entries) to O(visible-rows × columns).
    let row_height = CARD_HEIGHT + GRID_SPACING;
    let num_rows = order.len().div_ceil(columns);

    egui::ScrollArea::vertical()
        .id_salt("grid_scroll")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, num_rows, |ui, row_range| {
            // Inset so cards don't clip into toolbar or panel edges
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    for row in row_range {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = GRID_SPACING;
                            for col in 0..columns {
                                let idx = row * columns + col;
                                if idx >= order.len() {
                                    break;
                                }
                                let card_action =
                                    render_card(ui, theme, &entries[order[idx]], selection);
                                if action.is_none() {
                                    action = card_action;
                                }
                            }
                        });
                    }
                });
        });

    action
}

/// Render a single file/folder card
fn render_card(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entry: &FileEntry,
    selection: &mut RevisionedSelection,
) -> Option<FileListAction> {
    let mut action: Option<FileListAction> = None;

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(CARD_WIDTH, CARD_HEIGHT),
        egui::Sense::click_and_drag(),
    );

    if !ui.is_rect_visible(rect) {
        return None;
    }

    let hovered = response.hovered();
    let selected = selection.contains(&entry.path);

    // ── Background ──────────────────────────────────────────────
    let bg = if selected {
        theme.colors.selection
    } else if hovered {
        theme.colors.surface_variant
    } else {
        egui::Color32::TRANSPARENT
    };

    let stroke = if selected {
        egui::Stroke::new(1.0, theme.colors.primary)
    } else if hovered {
        egui::Stroke::new(1.0, theme.colors.outline)
    } else {
        egui::Stroke::NONE
    };

    ui.painter()
        .rect(rect, CARD_ROUNDING, bg, stroke, egui::StrokeKind::Outside);

    // ── Selection checkmark (top-right corner) ────────────────
    if selected {
        let check_center = pixel_align(egui::pos2(rect.max.x - 12.0, rect.min.y + 12.0));
        // Circle background
        ui.painter()
            .circle_filled(check_center, 8.0, theme.colors.primary);
        // Checkmark — use on_primary for guaranteed contrast
        ui.painter().text(
            check_center,
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::CHECK,
            egui::FontId::proportional(10.0),
            theme.colors.on_primary,
        );
    }

    // ── Icon area ───────────────────────────────────────────────
    let icon_center = pixel_align(egui::pos2(
        rect.center().x,
        rect.min.y + CARD_PADDING + ICON_SIZE * 0.5,
    ));

    let (icon_char, icon_color) = file_type_icon(&entry.name, entry.is_folder, theme);

    ui.painter().text(
        icon_center,
        egui::Align2::CENTER_CENTER,
        icon_char,
        egui::FontId::proportional(ICON_SIZE * 0.6),
        icon_color,
    );

    // Extension badge (for files only)
    if !entry.is_folder {
        let ext = extension(&entry.name).to_uppercase();
        if !ext.is_empty() && ext.len() <= 5 {
            let badge_pos =
                pixel_align(egui::pos2(icon_center.x, icon_center.y + ICON_SIZE * 0.38));
            ui.painter().text(
                badge_pos,
                egui::Align2::CENTER_CENTER,
                &ext,
                egui::FontId::monospace(9.0),
                theme.colors.on_surface_variant,
            );
        }
    }

    // ── Filename ────────────────────────────────────────────────
    let text_top = rect.min.y + CARD_PADDING + ICON_SIZE + 8.0;
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(rect.min.x + CARD_PADDING, text_top),
        egui::pos2(rect.max.x - CARD_PADDING, text_top + 28.0),
    );

    let name_color = if selected {
        theme.colors.on_surface
    } else {
        theme.colors.on_surface
    };

    // Truncate with ellipsis via egui's layout
    let galley = ui.painter().layout(
        entry.name.clone(),
        egui::FontId::proportional(11.0),
        name_color,
        text_rect.width(),
    );
    // Center the text horizontally
    let text_pos = pixel_align(egui::pos2(
        text_rect.center().x - galley.size().x * 0.5,
        text_rect.min.y,
    ));
    ui.painter().galley(text_pos, galley, name_color);

    // ── Size line ───────────────────────────────────────────────
    let size_y = text_top + 30.0;
    if size_y + 12.0 < rect.max.y {
        let size_text = if entry.is_folder {
            "Folder".to_string()
        } else {
            entry.size.clone()
        };
        ui.painter().text(
            pixel_align(egui::pos2(rect.center().x, size_y)),
            egui::Align2::CENTER_TOP,
            &size_text,
            egui::FontId::proportional(10.0),
            theme.colors.on_surface_variant,
        );
    }

    // ── Context menu (same as list view) ────────────────────────
    let entry_name = entry.name.clone();
    let is_folder = entry.is_folder;

    response.context_menu(|ui| {
        if ui
            .add(TextButton::new("📂  Open", ButtonSize::Medium).with_theme_colors(&theme.colors))
            .clicked()
        {
            if is_folder {
                action = Some(FileListAction::Navigate(entry_name.clone()));
            } else {
                action = Some(FileListAction::Open(entry_name.clone()));
            }
            ui.close();
        }
        ui.separator();
        if ui
            .add(
                TextButton::new("📦  Extract", ButtonSize::Medium).with_theme_colors(&theme.colors),
            )
            .clicked()
        {
            action = Some(FileListAction::Extract(entry_name.clone()));
            ui.close();
        }
        if ui
            .add(
                TextButton::new("📁  Extract To...", ButtonSize::Medium)
                    .with_theme_colors(&theme.colors),
            )
            .clicked()
        {
            action = Some(FileListAction::ExtractTo(entry_name.clone()));
            ui.close();
        }
        ui.separator();
        if ui
            .add(
                TextButton::new("📋  Copy Path", ButtonSize::Medium)
                    .with_theme_colors(&theme.colors),
            )
            .clicked()
        {
            action = Some(FileListAction::CopyPath(entry_name.clone()));
            ui.close();
        }
        ui.separator();
        if ui
            .add(
                TextButton::new("ℹ️  Properties", ButtonSize::Medium)
                    .with_theme_colors(&theme.colors),
            )
            .clicked()
        {
            action = Some(FileListAction::ShowProperties(entry_name.clone()));
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
            action = Some(FileListAction::Edit(entry_name.clone()));
            ui.close();
        }
        if ui
            .add(TextButton::new("🗑️  Delete", ButtonSize::Medium).with_theme_colors(&theme.colors))
            .clicked()
        {
            action = Some(FileListAction::Delete(entry_name.clone()));
            ui.close();
        }
    });

    // ── Interactions ────────────────────────────────────────────
    if response.drag_started() {
        response.ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        action = Some(FileListAction::DragStarted(vec![entry.name.clone()]));
    } else if response.double_clicked() {
        if entry.is_folder {
            action = Some(FileListAction::Navigate(entry.name.clone()));
        } else {
            action = Some(FileListAction::Open(entry.name.clone()));
        }
    } else if response.clicked() {
        if selection.contains(&entry.path) {
            selection.remove(&entry.path);
        } else {
            selection.insert(entry.path.clone());
        }
    }

    action
}

// ── File type icon mapping ──────────────────────────────────────────

fn file_type_icon<'a>(name: &str, is_folder: bool, theme: &AppTheme) -> (&'a str, egui::Color32) {
    let ext_colors = &theme.extensions;
    if is_folder {
        return (egui_phosphor::regular::FOLDER, ext_colors.file_folder);
    }

    let ext = extension(name).to_lowercase();
    match ext.as_str() {
        "exe" | "msi" | "bat" | "cmd" | "com" | "scr" => (
            egui_phosphor::regular::APP_WINDOW,
            ext_colors.file_executable,
        ),
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" => {
            (egui_phosphor::regular::FILE_ZIP, ext_colors.file_archive)
        }
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tiff" => {
            (egui_phosphor::regular::IMAGE, ext_colors.file_image)
        }
        "mp3" | "wav" | "ogg" | "flac" | "aac" | "wma" | "m4a" | "opus" => {
            (egui_phosphor::regular::MUSIC_NOTE, ext_colors.file_audio)
        }
        "mp4" | "avi" | "mkv" | "mov" | "wmv" | "flv" | "webm" => {
            (egui_phosphor::regular::FILM_STRIP, ext_colors.file_video)
        }
        // Text + word-processor docs share file_document.
        "txt" | "md" | "log" | "nfo" | "ini" | "cfg" | "conf" | "toml" | "yaml" | "yml" | "csv"
        | "tsv" | "doc" | "docx" | "odt" | "rtf" => {
            (egui_phosphor::regular::FILE_TEXT, ext_colors.file_document)
        }
        "rs" | "py" | "js" | "ts" | "html" | "css" | "json" | "xml" | "c" | "cpp" | "h"
        | "java" | "rb" | "go" | "sh" | "ps1" | "lua" => {
            (egui_phosphor::regular::FILE_CODE, ext_colors.file_code)
        }
        "pdf" => (egui_phosphor::regular::FILE_PDF, ext_colors.file_pdf),
        "url" | "lnk" | "desktop" => (egui_phosphor::regular::LINK, ext_colors.file_link),
        "dll" | "so" | "dylib" => (egui_phosphor::regular::PUZZLE_PIECE, ext_colors.file_link),
        "ttf" | "otf" | "woff" | "woff2" => {
            (egui_phosphor::regular::TEXT_AA, ext_colors.file_other)
        }
        "db" | "sqlite" | "sql" => (egui_phosphor::regular::DATABASE, ext_colors.file_folder),
        _ => (egui_phosphor::regular::FILE, ext_colors.file_other),
    }
}

fn extension(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or("")
}
