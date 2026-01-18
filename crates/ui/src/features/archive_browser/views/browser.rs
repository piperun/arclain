use crate::shared::components::{file_list, tree_panel};

use crate::features::archive_browser::{ArchiveBrowserAction, ArchiveBrowserState};
use crate::shared::SharedState;
use arclain_core::ActionType;
use arclain_plugins::types::PluginExtensionPoint;

pub fn render_archive_browser(
    ctx: &egui::Context,
    state: &mut ArchiveBrowserState,
    shared: &SharedState,
) -> ArchiveBrowserAction {
    let mut action = ArchiveBrowserAction::None;

    // Check if archive is loaded
    let archive_loaded = shared.signals().archive_path.get().is_some();

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

    // Update selection_count signal for toolbar button state
    // IMPORTANT: Only set if changed to avoid triggering repaint loop!
    let selection_count = state.entries.iter().filter(|e| e.selected).count();
    if shared.signals().selection_count.get() != selection_count {
        shared.signals().selection_count.set(selection_count);
    }

    // After UI has rendered, dispatch any pending plugin events.
    // This ensures plugins only receive archive_opened after the UI is ready.
    if !shared.signals().ui_ready.get() {
        let mut app_state = shared.app_state.lock();
        app_state.dispatch_pending_plugin_event();
    }

    action
}

fn render_empty_state(ctx: &egui::Context, shared: &SharedState) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(shared.theme.colors.surface))
        .show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("📦").size(64.0));
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("No archive loaded")
                            .size(18.0)
                            .color(shared.theme.colors.on_surface),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Click 'Open' to load an archive")
                            .size(14.0)
                            .color(shared.theme.colors.on_surface_variant),
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
        .frame(egui::Frame::NONE.fill(shared.theme.colors.surface_variant))
        .show(ctx, |ui| {
            let archive_name = shared
                .signals()
                .archive_path
                .get()
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "archive".to_string());

            let entries = shared.signals().entries.get();
            let folders = shared.signals().navigation.get().get_all_folders(&entries);
            let current_path = shared.signals().navigation.get().current_path.clone();

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
                .fill(shared.theme.colors.surface_variant)
                .inner_margin(egui::Margin::symmetric(16, 16)),
        )
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                // Read signals lock-free first
                let archive_info = shared.signals().archive_info.get();
                let items = shared.signals().info_panel_items.get();
                let plugin_metadata = shared.signals().metadata.get();

                let mut sections: Vec<properties_panel::PanelSection> = Vec::new();

                // Helper to get selected entry
                let selected_entries: Vec<_> =
                    state.entries.iter().filter(|e| e.selected).collect();
                let selected_entry = if selected_entries.len() == 1 {
                    Some(selected_entries[0])
                } else {
                    None
                };

                for item in items.iter().filter(|i| i.visible) {
                    match item.id.as_str() {
                        "info.archive" => {
                            if archive_info.archive_loaded {
                                sections.push(properties_panel::PanelSection::Group(
                                    properties_panel::create_archive_info_group(
                                        &archive_info.archive_format,
                                        archive_info.file_count,
                                        &format_size(archive_info.total_size),
                                        &format_size(archive_info.compressed_size),
                                        archive_info.total_crc32.as_deref(),
                                        archive_info.archive_encrypted,
                                        archive_info.headers_encrypted,
                                        archive_info.encryption_method.as_deref(),
                                    ),
                                ));
                            }
                        }
                        "info.file" => {
                            if let Some(entry) = selected_entry {
                                sections.push(properties_panel::PanelSection::Group(
                                    properties_panel::create_file_info_group(
                                        &entry.name,
                                        &entry.size,
                                        &entry.compressed,
                                        &entry.ratio,
                                    ),
                                ));
                            }
                        }
                        "info.attributes" => {
                            if let Some(entry) = selected_entry {
                                sections.push(properties_panel::PanelSection::Group(
                                    properties_panel::create_attributes_group(
                                        &entry.modified,
                                        &entry.crc32,
                                        if entry.encrypted { "Encrypted" } else { "None" },
                                    ),
                                ));
                            }
                        }
                        "info.plugin_metadata" => {
                            // Check signal first for reactive updates
                            let metadata = plugin_metadata
                                .clone()
                                .or_else(|| archive_info.plugin_metadata.clone());

                            if let Some(metadata) = metadata {
                                if let Some(group) =
                                    properties_panel::create_plugin_metadata_group(&metadata)
                                {
                                    sections.push(properties_panel::PanelSection::Group(group));
                                }
                            }
                        }
                        _ => {
                            // Check for plugin custom UI
                            if item.action_type == ActionType::Plugin {
                                if let Some(plugin_id) = &item.action_data {
                                    if let Some(manager_arc) = &shared.services.plugin_manager {
                                        let manager = manager_arc.lock();

                                        let elements = manager
                                            .with_plugin_instance(plugin_id, |instance| {
                                                instance
                                                    .get_ui_layout(PluginExtensionPoint::Panel)
                                                    .unwrap_or_default()
                                            })
                                            .unwrap_or_default();

                                        if !elements.is_empty() {
                                            let flat = elements.flatten();
                                            // Add to sections instead of rendering directly
                                            // This ensures plugins respect the layout order
                                            sections.push(properties_panel::PanelSection::Plugin {
                                                plugin_id: plugin_id.clone(),
                                                elements: flat,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Extract plugin_manager from services (no lock needed)
                let plugin_manager = shared.services.plugin_manager.clone();

                let panel_action = properties_panel::render(
                    ui,
                    &shared.theme,
                    &sections,
                    plugin_manager.as_ref(),
                    Some(shared),
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
        .frame(egui::Frame::NONE.fill(shared.theme.colors.surface))
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                // Get search text from signal
                let search_text = shared.signals().search_text.get();
                let search_lower = search_text.to_lowercase();
                let is_searching = !search_text.trim().is_empty();

                // Render file list
                egui::ScrollArea::vertical()
                    .id_salt("file_list_scroll")
                    .show(ui, |ui| {
                        if is_searching {
                            // Filtering mode: collect matching entries into a cloned Vec
                            let matching_indices: Vec<usize> = state
                                .entries
                                .iter()
                                .enumerate()
                                .filter(|(_, e)| e.name.to_lowercase().contains(&search_lower))
                                .map(|(i, _)| i)
                                .collect();

                            let mut filtered: Vec<_> = matching_indices
                                .iter()
                                .filter_map(|&i| state.entries.get(i).cloned())
                                .collect();

                            if state.toolbar_state.grid_view {
                                if let Some(file_action) =
                                    file_list::render_grid_view(ui, &shared.theme, &mut filtered)
                                {
                                    *action = map_file_list_action(file_action);
                                }
                            } else if let Some(file_action) = file_list::render_list_view(
                                ui,
                                &shared.theme,
                                &mut filtered,
                                state.toolbar_state.columns_locked,
                                &mut state.sort_state,
                            ) {
                                *action = map_file_list_action(file_action);
                            }

                            // Sync selection state back to original entries
                            for (filtered_idx, &original_idx) in matching_indices.iter().enumerate()
                            {
                                if let Some(filtered_entry) = filtered.get(filtered_idx) {
                                    if let Some(original_entry) =
                                        state.entries.get_mut(original_idx)
                                    {
                                        original_entry.selected = filtered_entry.selected;
                                    }
                                }
                            }
                        } else {
                            // Normal mode: render all entries
                            if state.toolbar_state.grid_view {
                                if let Some(file_action) = file_list::render_grid_view(
                                    ui,
                                    &shared.theme,
                                    &mut state.entries,
                                ) {
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
                        }
                    });
            });
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
        file_list::FileListAction::Extract(file) => ArchiveBrowserAction::Extract(file),
        file_list::FileListAction::ExtractTo(file) => ArchiveBrowserAction::ExtractTo(file),
        file_list::FileListAction::CopyPath(file) => ArchiveBrowserAction::CopyPath(file),
        file_list::FileListAction::ShowProperties(file) => {
            ArchiveBrowserAction::ShowProperties(file)
        }
        file_list::FileListAction::DragStarted(files) => ArchiveBrowserAction::DragExtract(files),
    }
}
