use eframe::egui;
use super::theme::AppTheme;

#[derive(Clone)]
pub struct FileEntry {
    pub name: String,
    pub size: String,
    pub compressed: String,
    pub ratio: String,
    pub modified: String,
    pub encrypted: bool,
    pub is_folder: bool,
    pub selected: bool,
}

pub fn render_breadcrumb(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    current_path: &str,
    archive_name: &str
) -> Option<String> {
    let mut navigate_to: Option<String> = None;
    
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);
        
        // Root archive button (clickable)
        let root_btn = egui::Button::new(
            egui::RichText::new(archive_name)
                .size(14.0)
                .color(theme.colors.text_primary)
        )
        .frame(false);
        
        if ui.add(root_btn).clicked() {
            navigate_to = Some(String::new()); // Navigate to root
        }
        
        if !current_path.is_empty() {
            ui.label(
                egui::RichText::new("/")
                    .size(14.0)
                    .color(theme.colors.text_muted)
            );
            
            // Split path into segments and make each clickable
            let segments: Vec<&str> = current_path.split('/').collect();
            for (idx, segment) in segments.iter().enumerate() {
                if idx > 0 {
                    ui.label(
                        egui::RichText::new("/")
                            .size(14.0)
                            .color(theme.colors.text_muted)
                    );
                }
                
                let segment_btn = egui::Button::new(
                    egui::RichText::new(*segment)
                        .size(14.0)
                        .color(if idx == segments.len() - 1 {
                            theme.colors.text_primary
                        } else {
                            theme.colors.text_secondary
                        })
                )
                .frame(false);
                
                if ui.add(segment_btn).clicked() {
                    // Build path up to this segment
                    let target_path = segments[..=idx].join("/");
                    navigate_to = Some(target_path);
                }
            }
        }
    });
    
    navigate_to
}

pub fn render_grid_view(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entries: &mut [FileEntry],
) -> Option<String> {
    let mut navigate_to: Option<String> = None;
    let available_width = ui.available_width();
    let item_width = 280.0;
    let columns = (available_width / item_width).floor().max(1.0) as usize;
    
    ui.spacing_mut().item_spacing = egui::vec2(1.0, 1.0);
    
    egui::Grid::new("file_grid")
        .num_columns(columns)
        .spacing([1.0, 1.0])
        .show(ui, |ui| {
            for (idx, entry) in entries.iter_mut().enumerate() {
                if idx > 0 && idx % columns == 0 {
                    ui.end_row();
                }
                
                let response = render_grid_item(ui, theme, entry);
                
                if response.clicked() {
                    entry.selected = !entry.selected;
                }
                
                if response.double_clicked() && entry.is_folder {
                    navigate_to = Some(entry.name.clone());
                }
            }
        });
    
    navigate_to
}

fn render_grid_item(ui: &mut egui::Ui, theme: &AppTheme, entry: &FileEntry) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(280.0, 60.0),
        egui::Sense::click(),
    );
    
    if ui.is_rect_visible(rect) {
        // Background
        let bg_color = if entry.selected {
            theme.colors.selection
        } else if response.hovered() {
            theme.colors.bg_hover
        } else {
            theme.colors.bg_primary
        };
        
        ui.painter().rect_filled(rect, 0.0, bg_color);
        
        // Content
        let content_rect = rect.shrink2(egui::vec2(12.0, 8.0));
        
        // Icon
        let icon_size = 32.0;
        let icon_rect = egui::Rect::from_min_size(
            content_rect.min,
            egui::vec2(icon_size, icon_size),
        );
        
        ui.painter().rect_filled(
            icon_rect,
            4.0,
            theme.colors.bg_tertiary,
        );
        
        let ext = entry.name.split('.').last().unwrap_or("").to_uppercase();
        let ext_text = if entry.is_folder { "📁" } else { &ext };
        
        ui.painter().text(
            icon_rect.center(),
            egui::Align2::CENTER_CENTER,
            ext_text,
            egui::FontId::proportional(12.0),
            theme.colors.text_muted,
        );
        
        // File info
        let text_x = content_rect.min.x + icon_size + 12.0;
        let name_pos = egui::pos2(text_x, content_rect.min.y + 8.0);
        let meta_pos = egui::pos2(text_x, content_rect.min.y + 28.0);
        
        // Use selection text color when selected
        let text_color = if entry.selected {
            theme.colors.selection_text
        } else {
            theme.colors.text_primary
        };
        
        let meta_color = if entry.selected {
            theme.colors.selection_text
        } else {
            theme.colors.text_muted
        };
        
        ui.painter().text(
            name_pos,
            egui::Align2::LEFT_TOP,
            &entry.name,
            egui::FontId::proportional(14.0),
            text_color,
        );
        
        ui.painter().text(
            meta_pos,
            egui::Align2::LEFT_TOP,
            format!("{} • {}", entry.size, entry.modified),
            egui::FontId::proportional(12.0),
            meta_color,
        );
    }
    
    response
}

pub fn render_list_view(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entries: &mut [FileEntry],
) -> Option<String> {
    use egui_extras::{TableBuilder, Column};
    
    let mut navigate_to: Option<String> = None;
    
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(30.0)) // Checkbox
        .column(Column::remainder().at_least(200.0)) // Name
        .column(Column::exact(100.0)) // Size
        .column(Column::exact(100.0)) // Compressed
        .column(Column::exact(70.0)) // Ratio
        .column(Column::exact(120.0)) // Modified
        .column(Column::exact(30.0)) // Encrypted
        .header(28.0, |mut header| {
            header.col(|_ui| {});
            header.col(|ui| {
                ui.label(egui::RichText::new("Name")
                    .size(12.0)
                    .strong()
                    .color(theme.colors.text_secondary));
            });
            header.col(|ui| {
                ui.label(egui::RichText::new("Size")
                    .size(12.0)
                    .strong()
                    .color(theme.colors.text_secondary));
            });
            header.col(|ui| {
                ui.label(egui::RichText::new("Compressed")
                    .size(12.0)
                    .strong()
                    .color(theme.colors.text_secondary));
            });
            header.col(|ui| {
                ui.label(egui::RichText::new("Ratio")
                    .size(12.0)
                    .strong()
                    .color(theme.colors.text_secondary));
            });
            header.col(|ui| {
                ui.label(egui::RichText::new("Modified")
                    .size(12.0)
                    .strong()
                    .color(theme.colors.text_secondary));
            });
            header.col(|_ui| {});
        })
        .body(|mut body| {
            for entry in entries.iter_mut() {
                let entry_name = entry.name.clone();
                let is_folder = entry.is_folder;
                
                body.row(32.0, |mut row| {
                    row.col(|ui| {
                        ui.checkbox(&mut entry.selected, "");
                    });
                    
                    row.col(|ui| {
                        let icon = if is_folder { "📁" } else { "📄" };
                        let text = format!("{} {}", icon, entry_name);
                        
                        let response = ui.selectable_label(false, 
                            egui::RichText::new(&text)
                                .size(14.0)
                                .color(theme.colors.text_primary)
                        );
                        
                        if response.double_clicked() && is_folder {
                            navigate_to = Some(entry_name.clone());
                        }
                    });
                    
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&entry.size)
                            .size(14.0)
                            .color(theme.colors.text_primary));
                    });
                    
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&entry.compressed)
                            .size(14.0)
                            .color(theme.colors.text_primary));
                    });
                    
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&entry.ratio)
                            .size(14.0)
                            .color(theme.colors.text_primary));
                    });
                    
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&entry.modified)
                            .size(14.0)
                            .color(theme.colors.text_primary));
                    });
                    
                    row.col(|ui| {
                        if entry.encrypted {
                            ui.label("🔒");
                        }
                    });
                });
            }
        });
    
    navigate_to
}