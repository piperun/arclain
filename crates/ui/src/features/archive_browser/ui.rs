use crate::shared::components::{file_list, tree_panel};

use super::{ArchiveBrowserAction, ArchiveBrowserState};
use crate::shared::SharedState;

pub fn render_archive_browser(
    ctx: &egui::Context,
    state: &mut ArchiveBrowserState,
    shared: &SharedState,
) -> ArchiveBrowserAction {
    let mut action = ArchiveBrowserAction::None;

    // Check if archive is loaded
    let archive_loaded = shared.app_state.lock().current_archive.is_some();

    if !archive_loaded {
        render_empty_state(ctx, shared);
        return action;
    }

    // Render tree panel if enabled
    if state.toolbar_state.show_tree_panel {
        render_tree_panel(ctx, state, shared, &mut action);
    }

    // Render properties panel if enabled
    if state.toolbar_state.show_properties_panel {
        if let Some(act) = render_properties_panel(ctx, state, shared) {
            action = act;
        }
    }

    // Render central file list
    render_file_list(ctx, state, shared, &mut action);

    action
}

fn render_empty_state(ctx: &egui::Context, shared: &SharedState) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(shared.theme.colors.bg_primary))
        .show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("📦").size(64.0));
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("No archive loaded")
                            .size(18.0)
                            .color(shared.theme.colors.text_primary),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Click 'Open' to load an archive")
                            .size(14.0)
                            .color(shared.theme.colors.text_secondary),
                    );
                });
            });
        });
}

fn render_tree_panel(
    ctx: &egui::Context,
    state: &mut ArchiveBrowserState,
    shared: &SharedState,
    action: &mut ArchiveBrowserAction,
) {
    egui::SidePanel::left("tree_panel")
        .exact_width(240.0)
        .frame(egui::Frame::NONE.fill(shared.theme.colors.bg_secondary))
        .show(ctx, |ui| {
            let app_state = shared.app_state.lock();
            let archive_name = app_state
                .current_archive
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "archive".to_string());

            let folders = app_state.navigation.get_all_folders(&app_state.all_entries);
            let current_path = app_state.navigation.current_path.clone();
            drop(app_state);

            if let Some(path) = tree_panel::render(
                ui,
                &shared.theme,
                &mut state.tree_state,
                &archive_name,
                &folders,
                &current_path,
            ) {
                *action = ArchiveBrowserAction::NavigateToPath(path);
            }
        });
}

fn render_properties_panel(
    ctx: &egui::Context,
    state: &ArchiveBrowserState,
    shared: &SharedState,
) -> Option<ArchiveBrowserAction> {
    use crate::core::utils::format_size;
    use crate::shared::components::{properties_panel, PropertiesPanelAction};

    let mut action = None;

    egui::SidePanel::right("properties_panel")
        .exact_width(280.0)
        .frame(
            egui::Frame::NONE
                .fill(shared.theme.colors.bg_secondary)
                .inner_margin(egui::Margin::symmetric(16, 16)),
        )
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let app_state = shared.app_state.lock();
                let archive_info = &app_state.archive_info;

                let mut groups = Vec::new();

                // Archive Info Group
                if archive_info.archive_loaded {
                    groups.push(properties_panel::create_archive_info_group(
                        &archive_info.archive_format,
                        archive_info.file_count,
                        &format_size(archive_info.total_size),
                        &format_size(archive_info.compressed_size),
                        archive_info.total_crc32.as_deref(),
                        archive_info.archive_encrypted,
                        archive_info.headers_encrypted,
                        archive_info.encryption_method.as_deref(),
                    ));
                }

                // Selected File Info
                let selected_entries: Vec<_> =
                    state.entries.iter().filter(|e| e.selected).collect();
                if selected_entries.len() == 1 {
                    let entry = selected_entries[0];
                    groups.push(properties_panel::create_file_info_group(
                        &entry.name,
                        &entry.size,
                        &entry.compressed,
                        &entry.ratio,
                    ));
                    groups.push(properties_panel::create_attributes_group(
                        &entry.modified,
                        &entry.crc32,
                        if entry.encrypted { "Encrypted" } else { "None" },
                    ));
                }

                // Plugin Metadata Group
                if let Some(metadata) = &archive_info.plugin_metadata {
                    if let Some(group) = properties_panel::create_plugin_metadata_group(metadata) {
                        groups.push(group);
                    }
                }

                drop(app_state);

                let panel_action = properties_panel::render(
                    ui,
                    &shared.theme,
                    &groups,
                    shared.app_state.lock().plugin_manager.as_ref(),
                );

                match panel_action {
                    PropertiesPanelAction::Organize => {
                        action = Some(ArchiveBrowserAction::Organize);
                    }
                    PropertiesPanelAction::Metadata(json) => {
                        action = Some(ArchiveBrowserAction::Metadata(json));
                    }
                    PropertiesPanelAction::None => {}
                }
            });
        });

    action
}

fn render_file_list(
    ctx: &egui::Context,
    state: &mut ArchiveBrowserState,
    shared: &SharedState,
    action: &mut ArchiveBrowserAction,
) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(shared.theme.colors.bg_primary))
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                // Render breadcrumb
                render_breadcrumb(ui, state, shared, action);

                // Render file list
                egui::ScrollArea::vertical()
                    .id_salt("file_list_scroll")
                    .show(ui, |ui| {
                        if state.toolbar_state.grid_view {
                            if let Some(file_action) =
                                file_list::render_grid_view(ui, &shared.theme, &mut state.entries)
                            {
                                *action = map_file_list_action(file_action);
                            }
                        } else if let Some(file_action) = file_list::render_list_view(
                            ui,
                            &shared.theme,
                            &mut state.entries,
                            state.toolbar_state.columns_locked,
                            &mut state.sort_state,
                        ) {
                            *action = map_file_list_action(file_action);
                        }
                    });
            });
        });
}

fn render_breadcrumb(
    ui: &mut egui::Ui,
    _state: &mut ArchiveBrowserState,
    shared: &SharedState,
    action: &mut ArchiveBrowserAction,
) {
    egui::Frame::NONE
        .fill(shared.theme.colors.bg_secondary)
        .inner_margin(egui::Margin::symmetric(16, 10))
        .stroke(egui::Stroke::new(1.0, shared.theme.colors.border_color))
        .show(ui, |ui| {
            let app_state = shared.app_state.lock();
            let archive_name = app_state
                .current_archive
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let current_path = app_state.navigation.current_path.clone();
            drop(app_state);

            if let Some(path) =
                file_list::render_breadcrumb(ui, &shared.theme, &current_path, &archive_name)
            {
                *action = ArchiveBrowserAction::NavigateToPath(path);
            }
        });
}

/// Common archive file extensions
const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz"];

/// Check if a filename has an archive extension
fn is_archive_file(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    ARCHIVE_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{}", ext)))
}

fn map_file_list_action(file_action: file_list::FileListAction) -> ArchiveBrowserAction {
    match file_action {
        file_list::FileListAction::Navigate(folder) => {
            ArchiveBrowserAction::NavigateToFolder(folder)
        }
        file_list::FileListAction::Open(file) => {
            // Check if the file is a nested archive
            if is_archive_file(&file) {
                ArchiveBrowserAction::OpenArchiveInTab(file)
            } else {
                ArchiveBrowserAction::OpenFile(file)
            }
        }
        file_list::FileListAction::Edit(file) => ArchiveBrowserAction::EditFile(file),
        file_list::FileListAction::Delete(file) => ArchiveBrowserAction::DeleteFile(file),
    }
}
