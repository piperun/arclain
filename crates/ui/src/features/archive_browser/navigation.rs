use super::ArchiveBrowserState;
use crate::core::utils::convert_to_file_entry;
use crate::shared::SharedState;
use std::path::PathBuf;

pub fn navigate_to_folder(state: &mut ArchiveBrowserState, shared: &SharedState, folder: &str) {
    let mut app_state = shared.app_state.lock();
    app_state.navigate_to_folder(folder);
    state.entries = app_state
        .get_current_entries()
        .iter()
        .map(convert_to_file_entry)
        .collect();
    state.current_path = update_current_path_display(
        app_state.navigation.current_path.clone(),
        app_state.current_archive.clone(),
    );
}

pub fn navigate_to_path(state: &mut ArchiveBrowserState, shared: &SharedState, path: &str) {
    let mut app_state = shared.app_state.lock();
    app_state.navigate_to_path(path);
    state.entries = app_state
        .get_current_entries()
        .iter()
        .map(convert_to_file_entry)
        .collect();
    state.current_path = update_current_path_display(
        app_state.navigation.current_path.clone(),
        app_state.current_archive.clone(),
    );
}

pub fn navigate_back(state: &mut ArchiveBrowserState, shared: &SharedState) {
    let mut app_state = shared.app_state.lock();
    if app_state.navigation.can_go_back() {
        app_state.navigate_back();
        state.entries = app_state
            .get_current_entries()
            .iter()
            .map(convert_to_file_entry)
            .collect();
        state.current_path = update_current_path_display(
            app_state.navigation.current_path.clone(),
            app_state.current_archive.clone(),
        );
    }
}

pub fn navigate_forward(state: &mut ArchiveBrowserState, shared: &SharedState) {
    let mut app_state = shared.app_state.lock();
    if app_state.navigation.can_go_forward() {
        app_state.navigate_forward();
        state.entries = app_state
            .get_current_entries()
            .iter()
            .map(convert_to_file_entry)
            .collect();
        state.current_path = update_current_path_display(
            app_state.navigation.current_path.clone(),
            app_state.current_archive.clone(),
        );
    }
}

pub fn navigate_up(state: &mut ArchiveBrowserState, shared: &SharedState) {
    let mut app_state = shared.app_state.lock();
    if app_state.navigation.can_go_up() {
        app_state.navigate_up();
        state.entries = app_state
            .get_current_entries()
            .iter()
            .map(convert_to_file_entry)
            .collect();
        state.current_path = update_current_path_display(
            app_state.navigation.current_path.clone(),
            app_state.current_archive.clone(),
        );
    }
}

fn update_current_path_display(path: String, archive: Option<PathBuf>) -> String {
    if let Some(archive_path) = archive {
        if let Some(archive_name) = archive_path.file_name() {
            if let Some(name) = archive_name.to_str() {
                if path.is_empty() {
                    return format!("{}/", name);
                } else {
                    return format!("{}/{}", name, path);
                }
            }
        }
    }
    path
}
