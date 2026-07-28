//! Main view for the archive browser feature.

use crate::core::tabs::view_state::{
    ArchiveTreeProjectionCache, BrowserProjectionCache, BrowserViewState,
};
use crate::core::tabs::TabState;
use crate::features::archive_browser::domain::Action;
use crate::features::archive_browser::presentation::components::file_list;
use crate::shared::components::tree_panel::{self, FolderTree, TreeRowProjectionCache};
use crate::shared::SharedState;
use arclain_core::ActionType;
use arclain_signals::Signal;
use eframe::egui;

/// Run cache-update work against a borrowed signal value, releasing the signal
/// lock before returning anything that will be consumed by egui rendering.
fn with_borrowed_signal_value<T, R>(signal: &Signal<T>, use_value: impl FnOnce(&T) -> R) -> R {
    let value = signal.read();
    let result = use_value(&value);
    drop(value);
    result
}

pub fn render_archive_browser(
    ctx: &egui::Context,
    shared: &SharedState,
    tab: &TabState,
    projection: &mut BrowserProjectionCache,
    tree_projection: &mut ArchiveTreeProjectionCache,
    tree_rows: &mut TreeRowProjectionCache,
) -> Action {
    let mut action = Action::None;

    let archive_loaded = tab.archive_loaded.get();

    if !archive_loaded {
        render_empty_state(ctx, shared);
        return action;
    }

    let entries = tab.browser_entries.get();
    let mut view_state = tab.browser_view_state.get();
    let render_projection =
        with_borrowed_signal_value(&shared.signals().search_text, |search_text| {
            projection.render_projection(
                &entries,
                view_state.sort_state,
                search_text,
                &view_state.selection,
            )
        });

    // Render tree panel if enabled
    if view_state.toolbar_state.show_tree_panel {
        let archive_path = tab.archive_path.get();
        let archive_name = archive_path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| std::borrow::Cow::Borrowed("archive"));
        let archive_entries = tab.entries.get();
        let navigation = tab.navigation.get();
        let tree = tree_projection.projection(&archive_entries, |entries| {
            FolderTree::from_folders(&navigation.get_all_folders(entries))
        });
        render_tree_panel(
            ctx,
            &mut view_state,
            shared,
            archive_name.as_ref(),
            tree.tree,
            tree.generation,
            tree_rows,
            &navigation.current_path,
            &mut action,
        );
    }

    // Render properties panel if enabled
    if view_state.toolbar_state.show_properties_panel {
        if let Some(act) = render_properties_panel(
            ctx,
            entries.entries.as_ref(),
            render_projection.selected_indices,
            shared,
            tab,
        ) {
            action = act;
        }
    }

    // Render central file list
    render_file_list(
        ctx,
        entries.entries.as_ref(),
        render_projection.visible_indices,
        render_projection.visible_selected_count,
        &mut view_state,
        shared,
        &mut action,
    );

    // Update selection_count signal for toolbar button state.
    // Selection lives in a dedicated HashSet now (post-refactor for
    // the worker-vs-renderer data race; see FileEntry docs).
    let selection_count = view_state.selection.len();
    if tab.selection_count.get() != selection_count {
        tab.selection_count.set(selection_count);
    }

    tab.browser_view_state.set_if_changed(view_state);

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
    state: &mut BrowserViewState,
    shared: &SharedState,
    archive_name: &str,
    tree: &FolderTree,
    tree_generation: u64,
    tree_rows: &mut TreeRowProjectionCache,
    current_path: &str,
    action: &mut Action,
) {
    egui::SidePanel::left("tree_panel")
        .exact_width(240.0)
        .frame(egui::Frame::NONE.fill(shared.theme.colors.surface_variant))
        .show(ctx, |ui| {
            if let Some(path) = tree_panel::render(
                ui,
                &shared.theme,
                &mut state.tree_state,
                archive_name,
                tree,
                tree_generation,
                tree_rows,
                current_path,
            ) {
                *action = Action::NavigateToPath(path);
            }
        });
}

fn render_properties_panel(
    ctx: &egui::Context,
    entries: &[crate::shared::models::file_entry::FileEntry],
    selected_indices: &[usize],
    shared: &SharedState,
    tab: &TabState,
) -> Option<Action> {
    use crate::core::utils::format_size;
    use crate::features::archive_browser::presentation::components::properties_panel::{
        self, PropertiesPanelAction,
    };

    let action = None;

    egui::SidePanel::right("properties_panel")
        .exact_width(280.0)
        .frame(
            egui::Frame::NONE
                .fill(shared.theme.colors.surface_variant)
                .inner_margin(egui::Margin::symmetric(16, 16)),
        )
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let archive_info = tab.archive_info.get();
                let items = shared.signals().info_panel_items.get();
                let plugin_metadata = tab.metadata.get();

                let mut sections: Vec<properties_panel::PanelSection> = Vec::new();

                let selected_entry = if selected_indices.len() == 1 {
                    Some(&entries[selected_indices[0]])
                } else {
                    None
                };

                // archive_loaded is its own Computed on TabState post
                // 2026-05-20 Tier 2 — `archive_info` (Computed<ArchiveInfo>)
                // no longer carries the flag.
                let archive_loaded = tab.archive_loaded.get();

                for item in items.iter().filter(|i| i.visible) {
                    match item.id.as_str() {
                        "info.archive" => {
                            if archive_loaded {
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
                            // Sourced from TabState::metadata (signal); the
                            // `archive_info.plugin_metadata` fallback was
                            // dropped in the 2026-05-20 Tier 2 cleanup
                            // (the field was always None).
                            if let Some(metadata) = plugin_metadata.clone() {
                                if let Some(group) =
                                    properties_panel::create_plugin_metadata_group(&metadata)
                                {
                                    sections.push(properties_panel::PanelSection::Group(group));
                                }
                            }
                        }
                        _ => {
                            if item.action_type == ActionType::Plugin {
                                if let Some(plugin_id) = &item.action_data {
                                    let origin_tab = Some(shared.signals().tabs.get().active_id());
                                    if let Some(Ok(layout)) = shared.plugin_ui_jobs.layout(
                                        plugin_id,
                                        crate::features::plugins::application::PluginUiTarget::Panel,
                                        origin_tab,
                                    ) {
                                        if !layout.is_empty() {
                                            sections.push(properties_panel::PanelSection::Plugin {
                                                plugin_id: plugin_id.clone(),
                                                elements: layout.as_ref().clone().flatten(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                let panel_action = properties_panel::render(
                    ui,
                    &shared.theme,
                    &sections,
                    Some(shared),
                );

                match panel_action {
                    PropertiesPanelAction::None => {}
                }
            });
        });

    action
}

fn render_file_list(
    ctx: &egui::Context,
    entries: &[crate::shared::models::file_entry::FileEntry],
    order: &[usize],
    visible_selected_count: usize,
    state: &mut BrowserViewState,
    shared: &SharedState,
    action: &mut Action,
) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(shared.theme.colors.surface))
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                // No outer ScrollArea here — both render_list_view (egui_extras
                // TableBuilder with body.rows virtualization) and render_grid_view
                // (ScrollArea::show_rows virtualization) own their own scrolling.
                // Wrapping them in an outer ScrollArea gives the virtualized
                // children infinite vertical room, which breaks the visible-row
                // computation and renders nothing.
                if state.toolbar_state.grid_view {
                    if let Some(file_action) = file_list::render_grid_view(
                        ui,
                        &shared.theme,
                        entries,
                        order,
                        &mut state.selection,
                    ) {
                        *action = map_file_list_action(file_action);
                    }
                } else if let Some(file_action) = file_list::render_list_view(
                    ui,
                    &shared.theme,
                    entries,
                    order,
                    visible_selected_count,
                    &mut state.selection,
                    state.toolbar_state.columns_locked,
                    &mut state.sort_state,
                ) {
                    *action = map_file_list_action(file_action);
                }
            });
        });
}

const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz"];

fn is_archive_file(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    ARCHIVE_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{}", ext)))
}

fn map_file_list_action(file_action: file_list::FileListAction) -> Action {
    match file_action {
        file_list::FileListAction::Navigate(folder) => Action::NavigateToFolder(folder),
        file_list::FileListAction::Open(file) => {
            if is_archive_file(&file) {
                Action::OpenArchiveInTab(file)
            } else {
                Action::OpenFile(file)
            }
        }
        file_list::FileListAction::Edit(file) => Action::EditFile(file),
        file_list::FileListAction::Delete(file) => Action::DeleteFile(file),
        file_list::FileListAction::Extract(file) => Action::Extract(file),
        file_list::FileListAction::ExtractTo(file) => Action::ExtractTo(file),
        file_list::FileListAction::CopyPath(file) => Action::CopyPath(file),
        file_list::FileListAction::ShowProperties(file) => Action::ShowProperties(file),
        file_list::FileListAction::DragStarted(files) => Action::DragExtract(files),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CloneTrackedSearch {
        value: String,
        clones: Arc<AtomicUsize>,
    }

    impl Clone for CloneTrackedSearch {
        fn clone(&self) -> Self {
            self.clones.fetch_add(1, Ordering::Relaxed);
            Self {
                value: self.value.clone(),
                clones: self.clones.clone(),
            }
        }
    }

    #[test]
    fn settled_nonempty_search_reads_do_not_clone_or_notify() {
        let clones = Arc::new(AtomicUsize::new(0));
        let notifications = Arc::new(AtomicUsize::new(0));
        let search = Signal::new(CloneTrackedSearch {
            value: "needle".to_string(),
            clones: clones.clone(),
        });
        let notification_count = notifications.clone();
        search.subscribe(move || {
            notification_count.fetch_add(1, Ordering::Relaxed);
        });

        for _ in 0..3 {
            with_borrowed_signal_value(&search, |query| {
                assert_eq!(query.value, "needle");
            });
        }

        assert_eq!(clones.load(Ordering::Relaxed), 0);
        assert_eq!(notifications.load(Ordering::Relaxed), 0);
    }
}
