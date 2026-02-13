//! Archive navigation state for UI
//!
//! This module provides navigation functionality for browsing archive contents.
//! May be moved to UI layer in the future.

use crate::archive::ArchiveEntry;

#[derive(Debug, Clone)]
pub struct NavigationState {
    pub current_path: String,
    pub path_stack: Vec<String>,
    pub forward_stack: Vec<String>,
}

impl NavigationState {
    pub fn new() -> Self {
        Self {
            current_path: String::new(),
            path_stack: vec![],
            forward_stack: vec![],
        }
    }

    pub fn navigate_to(&mut self, folder: &str) {
        let segment = Self::normalize_path(folder);
        if segment.is_empty() {
            return;
        }

        let current = Self::normalize_path(&self.current_path);
        if !current.is_empty() {
            self.path_stack.push(current.clone());
        }

        self.current_path = if current.is_empty() {
            segment
        } else {
            format!("{}/{}", current, segment)
        };
        self.forward_stack.clear();
    }

    pub fn navigate_to_absolute(&mut self, path: &str) {
        let new_path = Self::normalize_path(path); // Normalize but allow empty (root)

        // Don't navigate if path is same
        if new_path == self.current_path {
            return;
        }

        // Save current to history
        if !self.current_path.is_empty() {
            self.path_stack.push(self.current_path.clone());
        }

        self.current_path = new_path;
        self.forward_stack.clear();
    }

    pub fn navigate_back(&mut self) -> bool {
        if let Some(prev) = self.path_stack.pop() {
            self.forward_stack.push(self.current_path.clone());
            self.current_path = prev;
            true
        } else if !self.current_path.is_empty() {
            self.forward_stack.push(self.current_path.clone());
            self.current_path.clear();
            true
        } else {
            false
        }
    }

    pub fn navigate_forward(&mut self) -> bool {
        if let Some(next) = self.forward_stack.pop() {
            self.path_stack.push(self.current_path.clone());
            self.current_path = next;
            true
        } else {
            false
        }
    }

    pub fn navigate_up(&mut self) -> bool {
        if self.current_path.is_empty() {
            return false;
        }

        if let Some(pos) = self.current_path.rfind('/') {
            self.path_stack.push(self.current_path.clone());
            self.current_path = self.current_path[..pos].to_string();
            self.forward_stack.clear();
            true
        } else {
            self.path_stack.push(self.current_path.clone());
            self.current_path.clear();
            self.forward_stack.clear();
            true
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.path_stack.is_empty() || !self.current_path.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }

    pub fn can_go_up(&self) -> bool {
        !self.current_path.is_empty()
    }

    pub fn set_current_path(&mut self, path: &str) {
        self.current_path = Self::normalize_path(path);
    }

    pub fn get_all_folders(&self, entries: &[ArchiveEntry]) -> Vec<String> {
        let mut folders = std::collections::HashSet::new();

        for entry in entries {
            let normalized_path = Self::normalize_path(&entry.path);

            if entry.is_dir {
                folders.insert(normalized_path.clone());
            }

            let mut path = normalized_path;
            while let Some(pos) = path.rfind('/') {
                path = path[..pos].to_string();
                if !path.is_empty() {
                    folders.insert(path.clone());
                }
            }
        }

        let mut folder_vec: Vec<String> = folders.into_iter().collect();
        folder_vec.sort();
        folder_vec
    }

    pub fn filter_entries(&self, entries: &[ArchiveEntry]) -> Vec<ArchiveEntry> {
        let normalized_current = self.current_path.replace('\\', "/");
        let prefix = if normalized_current.is_empty() {
            String::new()
        } else {
            format!("{}/", normalized_current)
        };

        let items: Vec<ArchiveEntry> = entries
            .iter()
            .filter_map(|e| {
                let normalized_path = e.path.replace('\\', "/");

                if self.current_path.is_empty() {
                    if !normalized_path.contains('/') {
                        let mut entry = e.clone();
                        entry.path = normalized_path;
                        return Some(entry);
                    }

                    if let Some(pos) = normalized_path.find('/') {
                        let folder = normalized_path[..pos].to_string();
                        return Some(ArchiveEntry {
                            path: folder,
                            size: 0,
                            packed_size: 0,
                            modified: None,
                            is_dir: true,
                            encrypted: false,
                            crc32: None,
                        });
                    }

                    None
                } else if normalized_path.starts_with(&prefix) {
                    let relative = &normalized_path[prefix.len()..];
                    if relative.is_empty() {
                        return None;
                    }

                    if !relative.contains('/') {
                        let mut entry = e.clone();
                        entry.path = relative.to_string();
                        return Some(entry);
                    }

                    if let Some(pos) = relative.find('/') {
                        let folder = relative[..pos].to_string();
                        return Some(ArchiveEntry {
                            path: folder,
                            size: 0,
                            packed_size: 0,
                            modified: None,
                            is_dir: true,
                            encrypted: false,
                            crc32: None,
                        });
                    }

                    None
                } else {
                    None
                }
            })
            .collect();

        use std::collections::BTreeMap;

        let mut map: BTreeMap<String, ArchiveEntry> = BTreeMap::new();
        for entry in items {
            map.entry(entry.path.clone())
                .and_modify(|existing| {
                    if existing.modified.is_none() && entry.modified.is_some() {
                        *existing = entry.clone();
                    }
                })
                .or_insert(entry);
        }

        let mut result: Vec<ArchiveEntry> = map.into_values().collect();

        for entry in result.iter_mut().filter(|e| e.is_dir) {
            let full_path = if normalized_current.is_empty() {
                entry.path.clone()
            } else {
                format!("{}/{}", normalized_current, entry.path)
            };

            let (size, packed) = Self::compute_folder_totals(entries, &full_path);
            entry.size = size;
            entry.packed_size = packed;

            entry.crc32 = Self::compute_folder_crc(entries, &full_path);
        }

        result
    }

    fn compute_folder_totals(entries: &[ArchiveEntry], folder_path: &str) -> (u64, u64) {
        let normalized_folder = Self::normalize_path(folder_path);
        let prefix = format!("{}/", normalized_folder.trim_end_matches('/'));
        let mut size = 0u64;
        let mut packed = 0u64;

        for entry in entries {
            if entry.is_dir {
                continue;
            }
            let normalized = entry.path.replace('\\', "/");
            if normalized == normalized_folder || normalized.starts_with(&prefix) {
                size = size.saturating_add(entry.size);
                packed = packed.saturating_add(entry.packed_size);
            }
        }

        (size, packed)
    }

    fn compute_folder_crc(entries: &[ArchiveEntry], folder_path: &str) -> Option<String> {
        use crc32fast::Hasher;
        let normalized_folder = Self::normalize_path(folder_path);
        let prefix = format!("{}/", normalized_folder.trim_end_matches('/'));
        let mut items: Vec<(String, String)> = Vec::new();

        for entry in entries {
            if entry.is_dir {
                continue;
            }
            let normalized = entry.path.replace('\\', "/");
            if normalized == normalized_folder || normalized.starts_with(&prefix) {
                if let Some(crc) = &entry.crc32 {
                    items.push((normalized.clone(), crc.to_uppercase()));
                }
            }
        }

        if items.is_empty() {
            return None;
        }

        items.sort_by(|a, b| a.0.cmp(&b.0));
        let mut hasher = Hasher::new();
        for (p, c) in items {
            hasher.update(p.as_bytes());
            hasher.update(b":");
            hasher.update(c.as_bytes());
            hasher.update(b"\n");
        }
        let sum = hasher.finalize();
        Some(format!("{:08X}", sum))
    }

    fn normalize_path(path: &str) -> String {
        path.split(|c| c == '/' || c == '\\')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    }
}

impl Default for NavigationState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(path: &str, is_dir: bool) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_string(),
            size: if is_dir { 0 } else { 100 },
            packed_size: if is_dir { 0 } else { 80 },
            modified: None,
            is_dir,
            encrypted: false,
            crc32: None,
        }
    }

    // =========================================================================
    // Basic navigation
    // =========================================================================

    #[test]
    fn test_new_starts_at_root() {
        let nav = NavigationState::new();
        assert_eq!(nav.current_path, "");
        assert!(!nav.can_go_back());
        assert!(!nav.can_go_forward());
        assert!(!nav.can_go_up());
    }

    #[test]
    fn test_navigate_to() {
        let mut nav = NavigationState::new();
        nav.navigate_to("folder1");
        assert_eq!(nav.current_path, "folder1");
        assert!(nav.can_go_back());
        assert!(nav.can_go_up());
    }

    #[test]
    fn test_navigate_to_nested() {
        let mut nav = NavigationState::new();
        nav.navigate_to("folder1");
        nav.navigate_to("subfolder");
        assert_eq!(nav.current_path, "folder1/subfolder");
    }

    #[test]
    fn test_navigate_to_empty_is_noop() {
        let mut nav = NavigationState::new();
        nav.navigate_to("");
        assert_eq!(nav.current_path, "");
    }

    #[test]
    fn test_navigate_back() {
        let mut nav = NavigationState::new();
        nav.navigate_to("folder1");
        nav.navigate_to("subfolder");

        assert!(nav.navigate_back());
        assert_eq!(nav.current_path, "folder1");
        assert!(nav.can_go_forward());

        assert!(nav.navigate_back());
        assert_eq!(nav.current_path, "");
    }

    #[test]
    fn test_navigate_back_from_root_returns_false() {
        let mut nav = NavigationState::new();
        assert!(!nav.navigate_back());
    }

    #[test]
    fn test_navigate_forward() {
        let mut nav = NavigationState::new();
        nav.navigate_to("folder1");
        nav.navigate_back();
        assert!(nav.can_go_forward());

        assert!(nav.navigate_forward());
        assert_eq!(nav.current_path, "folder1");
        assert!(!nav.can_go_forward());
    }

    #[test]
    fn test_navigate_forward_empty_returns_false() {
        let nav = NavigationState::new();
        assert!(!nav.can_go_forward());
    }

    #[test]
    fn test_navigate_to_clears_forward_stack() {
        let mut nav = NavigationState::new();
        nav.navigate_to("a");
        nav.navigate_back();
        assert!(nav.can_go_forward());

        nav.navigate_to("b");
        assert!(!nav.can_go_forward());
    }

    #[test]
    fn test_navigate_up() {
        let mut nav = NavigationState::new();
        nav.set_current_path("a/b/c");

        assert!(nav.navigate_up());
        assert_eq!(nav.current_path, "a/b");

        assert!(nav.navigate_up());
        assert_eq!(nav.current_path, "a");

        assert!(nav.navigate_up());
        assert_eq!(nav.current_path, "");

        assert!(!nav.navigate_up());
    }

    #[test]
    fn test_navigate_to_absolute() {
        let mut nav = NavigationState::new();
        nav.navigate_to("folder1");
        nav.navigate_to_absolute("other/deep");
        assert_eq!(nav.current_path, "other/deep");
    }

    #[test]
    fn test_navigate_to_absolute_same_path_is_noop() {
        let mut nav = NavigationState::new();
        nav.navigate_to("folder1");
        let stack_len = nav.path_stack.len();
        nav.navigate_to_absolute("folder1");
        assert_eq!(nav.path_stack.len(), stack_len);
    }

    // =========================================================================
    // normalize_path
    // =========================================================================

    #[test]
    fn test_normalize_path() {
        assert_eq!(NavigationState::normalize_path("a/b/c"), "a/b/c");
        assert_eq!(NavigationState::normalize_path("a\\b\\c"), "a/b/c");
        assert_eq!(NavigationState::normalize_path("/a//b///c/"), "a/b/c");
        assert_eq!(NavigationState::normalize_path(""), "");
    }

    // =========================================================================
    // get_all_folders
    // =========================================================================

    #[test]
    fn test_get_all_folders() {
        let nav = NavigationState::new();
        let entries = vec![
            make_entry("game/data/file.dat", false),
            make_entry("game/Game.exe", false),
            make_entry("screenshots", true),
        ];
        let folders = nav.get_all_folders(&entries);
        assert!(folders.contains(&"game".to_string()));
        assert!(folders.contains(&"game/data".to_string()));
        assert!(folders.contains(&"screenshots".to_string()));
    }

    // =========================================================================
    // filter_entries
    // =========================================================================

    #[test]
    fn test_filter_entries_at_root() {
        let nav = NavigationState::new();
        let entries = vec![
            make_entry("readme.txt", false),
            make_entry("game/Game.exe", false),
            make_entry("game/data/file.dat", false),
        ];
        let filtered = nav.filter_entries(&entries);
        let paths: Vec<&str> = filtered.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"readme.txt"));
        assert!(paths.contains(&"game"));
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_entries_in_subfolder() {
        let mut nav = NavigationState::new();
        nav.set_current_path("game");
        let entries = vec![
            make_entry("readme.txt", false),
            make_entry("game/Game.exe", false),
            make_entry("game/data/file.dat", false),
        ];
        let filtered = nav.filter_entries(&entries);
        let paths: Vec<&str> = filtered.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"Game.exe"));
        assert!(paths.contains(&"data"));
        assert!(!paths.contains(&"readme.txt"));
    }
}
