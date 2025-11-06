use crate::app::state::AppState;
use crate::app::utils::convert_to_file_entry;
use crate::features::file_list;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

/// Navigate to a specific folder within the archive
pub fn navigate_to(
    state: &Arc<Mutex<AppState>>,
    folder: &str,
    entries: &mut Vec<file_list::FileEntry>,
    current_path: &mut String,
) {
    let mut st = state.lock();
    st.navigate_to_folder(folder);
    *entries = st
        .get_current_entries()
        .iter()
        .map(convert_to_file_entry)
        .collect();

    let nav_current_path = st.navigation.current_path.clone();
    let current_archive = st.current_archive.clone();
    drop(st);

    update_current_path(current_path, nav_current_path, current_archive);
}

/// Navigate back in the navigation history
pub fn navigate_back(
    state: &Arc<Mutex<AppState>>,
    entries: &mut Vec<file_list::FileEntry>,
    current_path: &mut String,
) {
    let mut st = state.lock();
    st.navigate_back();
    *entries = st
        .get_current_entries()
        .iter()
        .map(convert_to_file_entry)
        .collect();

    let nav_current_path = st.navigation.current_path.clone();
    let current_archive = st.current_archive.clone();
    drop(st);

    update_current_path(current_path, nav_current_path, current_archive);
}

/// Navigate forward in the navigation history
pub fn navigate_forward(
    state: &Arc<Mutex<AppState>>,
    entries: &mut Vec<file_list::FileEntry>,
    current_path: &mut String,
) {
    let mut st = state.lock();
    st.navigate_forward();
    *entries = st
        .get_current_entries()
        .iter()
        .map(convert_to_file_entry)
        .collect();

    let nav_current_path = st.navigation.current_path.clone();
    let current_archive = st.current_archive.clone();
    drop(st);

    update_current_path(current_path, nav_current_path, current_archive);
}

/// Navigate up one level in the folder hierarchy
pub fn navigate_up(
    state: &Arc<Mutex<AppState>>,
    entries: &mut Vec<file_list::FileEntry>,
    current_path: &mut String,
) {
    let mut st = state.lock();
    st.navigate_up();
    *entries = st
        .get_current_entries()
        .iter()
        .map(convert_to_file_entry)
        .collect();

    let nav_current_path = st.navigation.current_path.clone();
    let current_archive = st.current_archive.clone();
    drop(st);

    update_current_path(current_path, nav_current_path, current_archive);
}

/// Update the current path display string
pub fn update_current_path(
    current_path: &mut String,
    nav_current_path: String,
    current_archive: Option<PathBuf>,
) {
    *current_path = if nav_current_path.is_empty() {
        current_archive
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        format!(
            "{} > {}",
            current_archive
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy())
                .unwrap_or_default(),
            nav_current_path
        )
    };
}
