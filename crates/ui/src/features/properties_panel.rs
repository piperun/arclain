use eframe::egui;
use super::theme::AppTheme;

pub struct PropertyGroup {
    pub title: String,
    pub properties: Vec<(String, String)>,
}

pub fn render(ui: &mut egui::Ui, theme: &AppTheme, groups: &[PropertyGroup]) {
    ui.vertical(|ui| {
        ui.add_space(4.0);
        
        for (idx, group) in groups.iter().enumerate() {
            if idx > 0 {
                ui.add_space(8.0);
            }
            
            render_property_group(ui, theme, group);
        }
    });
}

fn render_property_group(ui: &mut egui::Ui, theme: &AppTheme, group: &PropertyGroup) {
    let group_frame = egui::Frame::none()
        .fill(theme.colors.bg_primary)
        .stroke(egui::Stroke::new(1.0, theme.colors.border_light))
        .rounding(4.0)
        .inner_margin(egui::Margin::symmetric(0.0, 12.0));
    
    group_frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        
        // Group title
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(&group.title)
                    .size(11.0)
                    .strong()
                    .color(theme.colors.text_muted)
            );
        });
        
        ui.add_space(8.0);
        
        // Properties
        for (label, value) in &group.properties {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                
                ui.label(
                    egui::RichText::new(label)
                        .size(14.0)
                        .color(theme.colors.text_secondary)
                );
                
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new(value)
                            .size(14.0)
                            .strong()
                            .color(theme.colors.text_primary)
                    );
                });
            });
            
            ui.add_space(4.0);
        }
    });
}

pub fn create_file_info_group(
    name: &str,
    size: &str,
    compressed: &str,
    ratio: &str,
) -> PropertyGroup {
    PropertyGroup {
        title: "FILE INFORMATION".to_string(),
        properties: vec![
            ("Name:".to_string(), name.to_string()),
            ("Size:".to_string(), size.to_string()),
            ("Compressed:".to_string(), compressed.to_string()),
            ("Ratio:".to_string(), ratio.to_string()),
        ],
    }
}

pub fn create_attributes_group(
    modified: &str,
    crc32: &str,
    method: &str,
) -> PropertyGroup {
    PropertyGroup {
        title: "ATTRIBUTES".to_string(),
        properties: vec![
            ("Modified:".to_string(), modified.to_string()),
            ("CRC32:".to_string(), crc32.to_string()),
            ("Method:".to_string(), method.to_string()),
        ],
    }
}

pub fn create_archive_info_group(
    format: &str,
    total_files: usize,
    total_size: &str,
    compressed_size: &str,
) -> PropertyGroup {
    PropertyGroup {
        title: "ARCHIVE INFO".to_string(),
        properties: vec![
            ("Total Files:".to_string(), total_files.to_string()),
            ("Total Size:".to_string(), total_size.to_string()),
            ("Compressed:".to_string(), compressed_size.to_string()),
            ("Format:".to_string(), format.to_string()),
        ],
    }
}