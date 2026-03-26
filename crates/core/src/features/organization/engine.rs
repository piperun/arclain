use super::{OrganizationRule, RuleTrigger};
use crate::ArchiveEntry;
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A pending download with cache information
#[derive(Debug, Clone)]
pub struct PendingDownload {
    /// DLsite product code (e.g., "RJ123456") if applicable
    pub product_id: Option<String>,
    /// Source URL to download from
    pub url: String,
    /// Destination path relative to root folder
    pub dest_path: String,
    /// Cache key for content cache lookup (e.g., "dlsite:RJ123456:screenshot_0")
    pub cache_key: String,
    /// Whether this content is already in cache
    pub cached: bool,
}

#[derive(Debug, Clone)]
pub struct OrganizationPlan {
    pub rule_name: String,
    pub root_folder: String,
    pub root_folder_template: String, // Original template pattern before variable expansion
    pub moves: Vec<(String, String)>, // (source_path, dest_path)
    pub generated_files: Vec<(String, String)>, // (path, content)
    pub downloads: Vec<PendingDownload>,
    pub use_standard_layout: bool,
    /// Resolved template variables for UI display (e.g., "code" -> "RJ123456")
    pub resolved_variables: HashMap<String, String>,
}

pub struct RuleEngine;

impl RuleEngine {
    /// Find all rules that match the given archive
    /// Find all rules that match the given archive
    pub fn find_matching_rules(
        rules: &[OrganizationRule],
        archive_name: &str,
        entries: &[ArchiveEntry],
        game_metadata: Option<&crate::features::organization::metadata::GameMetadata>,
    ) -> Vec<OrganizationRule> {
        let mut matches = Vec::new();

        for rule in rules {
            if !rule.is_enabled {
                continue;
            }

            if Self::matches_trigger(&rule.trigger, archive_name, entries, game_metadata) {
                matches.push(rule.clone());
            }
        }

        // Sort by priority (descending)
        matches.sort_by(|a, b| b.priority.cmp(&a.priority));
        matches
    }

    pub fn matches_trigger(
        trigger: &RuleTrigger,
        archive_name: &str,
        entries: &[ArchiveEntry],
        game_metadata: Option<&crate::features::organization::metadata::GameMetadata>,
    ) -> bool {
        // 1. Check metadata source trigger (Highest Priority)
        if let Some(source_trigger) = &trigger.metadata_source {
            if let Some(metadata) = game_metadata {
                if metadata.source.eq_ignore_ascii_case(source_trigger) {
                    return true;
                }
            }
            // If trigger requires metadata source but we don't have it or it doesn't match:
            // Do we fail immediately? Or fallback to regex?
            // "Trigger matching" implies ALL conditions must check out, OR specific ones override?
            // Usually, if a specific trigger is set, it MUST match.
            // But here "metadata_source" implies "If this matches, rule applies".
            // If it DOESN'T match, we should probably return FALSE immediately if we treat it as a constraint.
            // "I want this rule to apply only for DLsite games".
            // So if `source_trigger` is set, and metadata source != trigger, return false.
            if let Some(metadata) = game_metadata {
                if !metadata.source.eq_ignore_ascii_case(source_trigger) {
                    return false;
                }
            } else {
                // Trigger requires metadata, but we have none. Match failed.
                return false;
            }
        }

        // Check filename pattern
        if let Some(pattern) = &trigger.filename_pattern {
            if let Ok(re) = Regex::new(pattern) {
                if !re.is_match(archive_name) {
                    return false;
                }
            } else {
                return false; // Invalid regex
            }
        }

        // Check file existence
        if let Some(file_glob) = &trigger.has_file {
            // Simple check: does any entry path contain this string?
            // Real glob matching would be better, but for now simple contains/ends_with
            let found = entries.iter().any(|e| e.path.contains(file_glob));
            if !found {
                return false;
            }
        }

        true
    }

    /// Generate an organization plan based on a rule
    pub fn create_plan(
        rule: &OrganizationRule,
        archive_name: &str,
        entries: &[ArchiveEntry],
        game_metadata: Option<&crate::features::organization::metadata::GameMetadata>,
    ) -> Result<OrganizationPlan> {
        // Prune unnecessary files/folders first
        let pruned_entries = Self::prune_entries(entries);
        let entries = &pruned_entries;

        let mut moves = Vec::new();
        let mut metadata = HashMap::new();

        // 1. Populate from GameMetadata if available
        if let Some(gm) = game_metadata {
            metadata.insert("product_id".to_string(), gm.product_id.clone());
            metadata.insert("source".to_string(), gm.source.clone());
            metadata.insert("title".to_string(), gm.title.clone());

            // NEW: Add filtered_title for safe folder names
            let filtered = crate::utilities::title_filter::sanitize_title(&gm.title);
            metadata.insert("filtered_title".to_string(), filtered);

            if let Some(creator) = &gm.creator {
                metadata.insert("creator".to_string(), creator.clone());
                metadata.insert("circle".to_string(), creator.clone()); // Alias
            }
            if let Some(date) = &gm.release_date {
                metadata.insert("release_date".to_string(), date.clone());
            }

            // Parse JSON for platform-specific fields (generic flattening)
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&gm.metadata_json) {
                crate::features::organization::flatten_helper::flatten_json_value(
                    &json,
                    &mut metadata,
                    "",
                );
            }
        }

        // 2. Extract metadata from filename if pattern exists (overrides/supplements)
        if let Some(pattern) = &rule.trigger.filename_pattern {
            if let Ok(re) = Regex::new(pattern) {
                if let Some(caps) = re.captures(archive_name) {
                    for name in re.capture_names().flatten() {
                        if let Some(m) = caps.name(name) {
                            metadata.insert(name.to_string(), m.as_str().to_string());
                        }
                    }
                }
            }
        }

        // 3. Extract version from filename
        if let Ok(re) = Regex::new(r"[vV](\d+(\.\d+)+)") {
            if let Some(caps) = re.captures(archive_name) {
                if let Some(v) = caps.get(1) {
                    metadata.insert("version".to_string(), v.as_str().to_string());
                }
            }
        }

        // Detect content root early if sanitization is enabled
        let content_root = if rule.actions.use_standard_layout {
            Some(Self::find_game_content_root_in_entries(entries))
        } else {
            None
        };

        // If we found a content root (e.g. "Game_v1.0_[Patched]"), try to extract useful info from its name
        if let Some(root_path) = &content_root {
            if let Some(folder_name) = root_path.file_name().and_then(|n| n.to_str()) {
                // Extract Version from folder name
                if let Ok(re) = Regex::new(r"[vV](\d+(\.\d+)+)") {
                    if let Some(caps) = re.captures(folder_name) {
                        if let Some(v) = caps.get(1) {
                            metadata.insert("version".to_string(), v.as_str().to_string());
                        }
                    }
                }

                // Extract [TaGs] from folder name
                if let Ok(re) = Regex::new(r"\[([^\]]+)\]") {
                    let mut tags = Vec::new();
                    for cap in re.captures_iter(folder_name) {
                        if let Some(m) = cap.get(1) {
                            tags.push(m.as_str().to_string());
                        }
                    }
                    if !tags.is_empty() {
                        metadata.insert("root_tags".to_string(), tags.join(", "));
                        metadata.insert("folder_name".to_string(), folder_name.to_string());
                    }
                }
            }
        }

        // Determine root folder name
        let root_folder = if let Some(root_template) = &rule.actions.root_folder {
            Self::expand_variables(root_template, &metadata)
        } else {
            "Game".to_string()
        };

        // Process file moves
        if let Some(content_root) = content_root {
            // Sanitization Mode: We already found the root, use it to flatten
            // Put game content inside a "Game" subfolder within the root_folder
            let content_root_path = Path::new(&content_root);

            for entry in entries {
                if entry.is_dir {
                    continue;
                }

                // Only include files that are inside the content root
                // (This filters out junk wrappers efficiently)
                if let Ok(relative_content_path) =
                    Path::new(&entry.path).strip_prefix(content_root_path)
                {
                    // Add "Game/" prefix to put content in a subfolder
                    let dest_path = format!(
                        "{}/Game/{}",
                        root_folder,
                        relative_content_path.to_string_lossy()
                    );
                    let new_path = dest_path.replace("//", "/").replace("\\", "/");
                    moves.push((entry.path.clone(), new_path));
                }
            }
        } else if !rule.actions.use_standard_layout {
            // 1. Find common root directory to handle nested archives properly
            // e.g. if everything is in "GameName/...", we want to strip "GameName/"
            let paths: Vec<&Path> = entries.iter().map(|e| Path::new(&e.path)).collect();
            let common_root = if paths.is_empty() {
                PathBuf::new()
            } else {
                let mut iter = paths.iter();
                let mut root = iter
                    .next()
                    .unwrap()
                    .parent()
                    .unwrap_or(Path::new(""))
                    .to_path_buf();

                for path in iter {
                    while !path.starts_with(&root) {
                        if !root.pop() {
                            break;
                        }
                    }
                }
                root
            };

            for entry in entries {
                if entry.is_dir {
                    continue;
                }

                let mut target_dir = "game/".to_string(); // Default fallback

                // Find matching move rule
                for move_rule in &rule.actions.move_files {
                    if Self::matches_glob(&move_rule.pattern, &entry.path) {
                        target_dir = move_rule.target.clone();
                        break;
                    }
                }

                // Expand variables in target
                target_dir = Self::expand_variables(&target_dir, &metadata);

                // Construct new path: root_folder / target_dir / relative_path
                // Instead of taking just filename, we take path relative to common_root
                let relative_path = Path::new(&entry.path)
                    .strip_prefix(&common_root)
                    .unwrap_or(Path::new(&entry.path));

                // If the relative path is empty (shouldn't happen for files) or just filename, it works.
                // If it has subdirs, they are preserved.

                let dest_path = if target_dir.is_empty() || target_dir == "." {
                    format!("{}/{}", root_folder, relative_path.to_string_lossy())
                } else {
                    format!(
                        "{}/{}/{}",
                        root_folder,
                        target_dir,
                        relative_path.to_string_lossy()
                    )
                };

                let new_path = dest_path.replace("//", "/").replace("\\", "/");

                moves.push((entry.path.clone(), new_path));
            }
        }

        // Generate metadata.json if metadata is available
        let mut generated_files = Vec::new();
        if let Some(gm) = game_metadata {
            if let Ok(json_str) = serde_json::to_string_pretty(gm) {
                generated_files.push((format!("{}/metadata.json", root_folder), json_str));
            }
        }

        // Add screenshots to downloads
        let mut downloads = Vec::new();
        if let Some(gm) = game_metadata {
            let is_dlsite = gm.source.eq_ignore_ascii_case("dlsite");

            for (i, screenshot) in gm.screenshots.iter().enumerate() {
                if let crate::features::organization::metadata::ScreenshotData::FilePath(path) =
                    screenshot
                {
                    let url = path.to_string_lossy().to_string();
                    // Determine extension from URL or default to jpg
                    let ext = Path::new(&url)
                        .extension()
                        .map(|e| e.to_string_lossy().to_string())
                        .unwrap_or_else(|| "jpg".to_string());

                    let filename = format!("image_{:03}.{}", i + 1, ext);
                    // Standard layout uses lowercase "screenshots"
                    let screenshots_folder = if rule.actions.use_standard_layout {
                        "screenshots"
                    } else {
                        "Screenshots"
                    };
                    let dest_path = format!("{}/{}/{}", root_folder, screenshots_folder, filename);

                    // Cache key must match gameta's cache_keys format
                    let cache_key = if is_dlsite {
                        format!("dlsite:{}:screenshot_{}", gm.product_id, i)
                    } else {
                        format!("screenshot:{}:{}", gm.product_id, i)
                    };

                    downloads.push(PendingDownload {
                        product_id: if is_dlsite {
                            Some(gm.product_id.clone())
                        } else {
                            None
                        },
                        url,
                        dest_path,
                        cache_key,
                        cached: false, // Will be checked by UI when loading
                    });
                }
            }
        }

        // Store the original template for UI display
        let root_folder_template = rule
            .actions
            .root_folder
            .clone()
            .unwrap_or_else(|| "Game".to_string());

        Ok(OrganizationPlan {
            rule_name: rule.name.clone(),
            root_folder,
            root_folder_template,
            moves,
            generated_files,
            downloads,
            use_standard_layout: rule.actions.use_standard_layout,
            resolved_variables: metadata,
        })
    }

    fn expand_variables(template: &str, metadata: &HashMap<String, String>) -> String {
        let mut result = template.to_string();

        // Special handling for version prefix " v$version"
        if result.contains(" v$version") {
            if let Some(ver) = metadata.get("version") {
                result = result.replace(" v$version", &format!(" v{}", ver));
            } else {
                result = result.replace(" v$version", "");
            }
        }

        for (key, value) in metadata {
            let placeholder = format!("${}", key);
            result = result.replace(&placeholder, value);
        }

        // Clean up any remaining unreplaced variables if needed?
        // For now, leave them or maybe strip them?
        // User might want to see if something failed.

        result
    }

    fn matches_glob(pattern: &str, path: &str) -> bool {
        // Simple glob implementation or use `glob` crate if available
        // For now, support basic wildcards
        if pattern == "**" {
            return true;
        }

        // Use glob crate pattern matching if possible, or simple extension check
        // Here we'll do a simple extension check for *.ext
        if pattern.starts_with("*.") {
            let ext = &pattern[1..];
            return path.to_lowercase().ends_with(&ext.to_lowercase());
        }

        // Exact match
        if pattern == path {
            return true;
        }

        // Filename match
        if let Some(name) = Path::new(path).file_name() {
            if name.to_string_lossy() == pattern {
                return true;
            }
        }

        false
    }

    /// Prune unnecessary files (0-byte) and empty folders recursively
    /// Does NOT modify paths or filter "junk" - only removes empty files and directories
    pub(crate) fn prune_entries(entries: &[ArchiveEntry]) -> Vec<ArchiveEntry> {
        // 1. Build Tree
        let mut root = TreeNode::new(true);

        for entry in entries {
            root.insert(&entry.path, entry.clone());
        }

        // 2. Prune Tree (0-byte files and empty folders)
        root.prune();

        // 3. Flatten Tree
        root.flatten()
    }

    /// Helper to find the "game content" root folder in entries
    /// Mimics logic from flatten.rs::find_and_flatten_game_content
    fn find_game_content_root_in_entries(entries: &[ArchiveEntry]) -> PathBuf {
        let game_indicators = [
            "Game.exe",
            "game.exe",
            "nw.exe",
            "index.html",
            "package.json",
            "www",
            "data",
            "js",
        ];

        let mut best_root = PathBuf::new();
        let mut best_score = 0;

        // Group entries by parent directory - track both standard indicators and any .exe
        let mut dirs: HashMap<PathBuf, usize> = HashMap::new();
        let mut dirs_with_exe: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::new();

        for entry in entries {
            let path = Path::new(&entry.path);
            if let Some(parent) = path.parent() {
                if let Some(fname) = path.file_name() {
                    let fname_str = fname.to_string_lossy();

                    // Check for standard indicators
                    if game_indicators
                        .iter()
                        .any(|i| fname_str.eq_ignore_ascii_case(i))
                    {
                        *dirs.entry(parent.to_path_buf()).or_insert(0) += 1;
                    }

                    // Check for any .exe file (flexible indicator)
                    if !entry.is_dir && fname_str.to_lowercase().ends_with(".exe") {
                        dirs_with_exe.insert(parent.to_path_buf());
                    }
                }
            }
        }

        // Any .exe file counts as +1 indicator for that directory
        for dir in dirs_with_exe {
            *dirs.entry(dir).or_insert(0) += 1;
        }

        // Find dir with >= 2 indicators
        for (dir, score) in dirs {
            if score >= 2 && score > best_score {
                best_score = score;
                best_root = dir;
            }
        }

        // If no definitive root found (score < 2), fallback to common root or just root
        if best_score < 2 {
            // Find common root logic could be reused here or simple fallback
            // For now, if we can't find game content, we assume content is at root
            // of the *entries* (common prefix)
            let paths: Vec<&Path> = entries.iter().map(|e| Path::new(&e.path)).collect();
            if !paths.is_empty() {
                let mut iter = paths.iter();
                let mut root = iter
                    .next()
                    .unwrap()
                    .parent()
                    .unwrap_or(Path::new(""))
                    .to_path_buf();
                for path in iter {
                    while !path.starts_with(&root) {
                        if !root.pop() {
                            break;
                        }
                    }
                }
                return root;
            }
        }

        best_root
    }
}

struct TreeNode {
    entry: Option<ArchiveEntry>,
    children: HashMap<String, TreeNode>,
    is_dir: bool,
}

impl TreeNode {
    fn new(is_dir: bool) -> Self {
        Self {
            entry: None,
            children: HashMap::new(),
            is_dir,
        }
    }

    fn insert(&mut self, path: &str, entry: ArchiveEntry) {
        let parts: Vec<&str> = path
            .split(|c| c == '/' || c == '\\')
            .filter(|s| !s.is_empty())
            .collect();
        self.insert_recursive(&parts, entry);
    }

    fn insert_recursive(&mut self, parts: &[&str], entry: ArchiveEntry) {
        if parts.is_empty() {
            // This node represents the entry itself
            self.is_dir = entry.is_dir;
            self.entry = Some(entry);
            return;
        }

        let name = parts[0];
        let child = self
            .children
            .entry(name.to_string())
            .or_insert_with(|| TreeNode::new(true));
        child.insert_recursive(&parts[1..], entry);
    }

    fn prune(&mut self) -> bool {
        // Returns true if this node should be kept, false if it should be removed

        // 1. Prune children first (bottom-up)
        let mut to_remove = Vec::new();
        for (name, child) in &mut self.children {
            if !child.prune() {
                to_remove.push(name.clone());
            }
        }

        for name in to_remove {
            self.children.remove(&name);
        }

        // 2. Check if this node is unnecessary

        // If it's a file
        if !self.is_dir {
            if let Some(entry) = &self.entry {
                if entry.size == 0 {
                    return false; // Remove 0-byte file
                }
            }
            return true; // Keep non-zero file
        }

        // If it's a directory
        if self.children.is_empty() {
            // Empty directory -> Remove
            return false;
        }

        true // Keep directory with children
    }

    fn flatten(&self) -> Vec<ArchiveEntry> {
        let mut result = Vec::new();

        // Only include files, not directories
        if let Some(entry) = &self.entry {
            if !entry.is_dir {
                result.push(entry.clone());
            }
        }

        for child in self.children.values() {
            result.extend(child.flatten());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::organization::metadata::GameMetadata;
    use crate::features::organization::RuleActions;

    fn make_entry(path: &str, size: u64, is_dir: bool) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_string(),
            size,
            packed_size: size,
            modified: None,
            is_dir,
            encrypted: false,
            crc32: None,
        }
    }

    // =========================================================================
    // matches_trigger
    // =========================================================================

    #[test]
    fn test_matches_trigger_empty_matches_everything() {
        let trigger = RuleTrigger::default();
        assert!(RuleEngine::matches_trigger(&trigger, "anything.7z", &[], None));
    }

    #[test]
    fn test_matches_trigger_filename_pattern_match() {
        let trigger = RuleTrigger {
            filename_pattern: Some(r"RJ\d+".to_string()),
            ..Default::default()
        };
        assert!(RuleEngine::matches_trigger(
            &trigger,
            "RJ123456.7z",
            &[],
            None
        ));
        assert!(!RuleEngine::matches_trigger(
            &trigger,
            "something.zip",
            &[],
            None
        ));
    }

    #[test]
    fn test_matches_trigger_has_file() {
        let trigger = RuleTrigger {
            has_file: Some("Game.exe".to_string()),
            ..Default::default()
        };
        let entries = vec![
            make_entry("folder/Game.exe", 1024, false),
            make_entry("folder/readme.txt", 100, false),
        ];
        assert!(RuleEngine::matches_trigger(
            &trigger,
            "test.7z",
            &entries,
            None
        ));

        let no_match = vec![make_entry("folder/readme.txt", 100, false)];
        assert!(!RuleEngine::matches_trigger(
            &trigger,
            "test.7z",
            &no_match,
            None
        ));
    }

    #[test]
    fn test_matches_trigger_metadata_source() {
        let trigger = RuleTrigger {
            metadata_source: Some("dlsite".to_string()),
            ..Default::default()
        };
        let meta = GameMetadata {
            product_id: "RJ123".to_string(),
            source: "dlsite".to_string(),
            title: "Test".to_string(),
            description: None,
            tags: vec![],
            release_date: None,
            creator: None,
            screenshots: vec![],
            metadata_json: String::new(),
        };
        assert!(RuleEngine::matches_trigger(
            &trigger,
            "test.7z",
            &[],
            Some(&meta)
        ));

        // Wrong source
        let steam = GameMetadata {
            source: "steam".to_string(),
            ..meta.clone()
        };
        assert!(!RuleEngine::matches_trigger(
            &trigger,
            "test.7z",
            &[],
            Some(&steam)
        ));

        // No metadata at all
        assert!(!RuleEngine::matches_trigger(
            &trigger,
            "test.7z",
            &[],
            None
        ));
    }

    #[test]
    fn test_matches_trigger_invalid_regex_returns_false() {
        let trigger = RuleTrigger {
            filename_pattern: Some("[invalid".to_string()),
            ..Default::default()
        };
        assert!(!RuleEngine::matches_trigger(
            &trigger,
            "test.7z",
            &[],
            None
        ));
    }

    // =========================================================================
    // find_matching_rules
    // =========================================================================

    #[test]
    fn test_find_matching_rules_sorts_by_priority() {
        let rules = vec![
            OrganizationRule {
                name: "Low".to_string(),
                priority: 1,
                is_enabled: true,
                trigger: RuleTrigger::default(),
                ..Default::default()
            },
            OrganizationRule {
                name: "High".to_string(),
                priority: 100,
                is_enabled: true,
                trigger: RuleTrigger::default(),
                ..Default::default()
            },
        ];
        let matches = RuleEngine::find_matching_rules(&rules, "test.7z", &[], None);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].name, "High");
        assert_eq!(matches[1].name, "Low");
    }

    #[test]
    fn test_find_matching_rules_skips_disabled() {
        let rules = vec![OrganizationRule {
            name: "Disabled".to_string(),
            is_enabled: false,
            trigger: RuleTrigger::default(),
            ..Default::default()
        }];
        let matches = RuleEngine::find_matching_rules(&rules, "test.7z", &[], None);
        assert!(matches.is_empty());
    }

    // =========================================================================
    // expand_variables
    // =========================================================================

    #[test]
    fn test_expand_variables_basic() {
        let mut vars = HashMap::new();
        vars.insert("code".to_string(), "RJ123456".to_string());
        vars.insert("title".to_string(), "My Game".to_string());

        assert_eq!(
            RuleEngine::expand_variables("[$code] $title", &vars),
            "[RJ123456] My Game"
        );
    }

    #[test]
    fn test_expand_variables_version_present() {
        let mut vars = HashMap::new();
        vars.insert("version".to_string(), "1.2.3".to_string());

        assert_eq!(
            RuleEngine::expand_variables("Game v$version", &vars),
            "Game v1.2.3"
        );
    }

    #[test]
    fn test_expand_variables_version_absent_is_stripped() {
        let vars = HashMap::new();
        assert_eq!(
            RuleEngine::expand_variables("Game v$version", &vars),
            "Game"
        );
    }

    #[test]
    fn test_expand_variables_no_match_leaves_placeholder() {
        let vars = HashMap::new();
        assert_eq!(
            RuleEngine::expand_variables("$unknown", &vars),
            "$unknown"
        );
    }

    // =========================================================================
    // matches_glob
    // =========================================================================

    #[test]
    fn test_matches_glob_wildcard_all() {
        assert!(RuleEngine::matches_glob("**", "anything/at/all.txt"));
    }

    #[test]
    fn test_matches_glob_extension() {
        assert!(RuleEngine::matches_glob("*.exe", "game/Game.exe"));
        assert!(RuleEngine::matches_glob("*.exe", "Game.EXE"));
        assert!(!RuleEngine::matches_glob("*.exe", "readme.txt"));
    }

    #[test]
    fn test_matches_glob_exact_match() {
        assert!(RuleEngine::matches_glob("readme.txt", "readme.txt"));
        assert!(!RuleEngine::matches_glob("readme.txt", "other.txt"));
    }

    #[test]
    fn test_matches_glob_filename_match() {
        assert!(RuleEngine::matches_glob(
            "Game.exe",
            "folder/subfolder/Game.exe"
        ));
    }

    // =========================================================================
    // prune_entries
    // =========================================================================

    #[test]
    fn test_prune_removes_zero_byte_files() {
        let entries = vec![
            make_entry("game/Game.exe", 1024, false),
            make_entry("game/empty.txt", 0, false),
        ];
        let pruned = RuleEngine::prune_entries(&entries);
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].path, "game/Game.exe");
    }

    #[test]
    fn test_prune_removes_empty_directories() {
        let entries = vec![
            make_entry("game", 0, true),
            make_entry("game/Game.exe", 1024, false),
            make_entry("empty_dir", 0, true),
        ];
        let pruned = RuleEngine::prune_entries(&entries);
        // Only the file should remain (directories are filtered in flatten)
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].path, "game/Game.exe");
    }

    #[test]
    fn test_prune_keeps_nonzero_files() {
        let entries = vec![
            make_entry("a.txt", 10, false),
            make_entry("b.txt", 20, false),
        ];
        let pruned = RuleEngine::prune_entries(&entries);
        assert_eq!(pruned.len(), 2);
    }

    // =========================================================================
    // screenshot cache keys (regression)
    // =========================================================================

    /// Regression: screenshot cache keys must use `gm.product_id` directly
    /// so they match the keys produced by the gameta server cache.
    /// Previously the code looked up a "code" key in the metadata hashmap,
    /// which could differ from the actual product_id.
    #[test]
    fn test_screenshot_cache_keys_use_product_id_for_dlsite() {
        use crate::features::organization::metadata::ScreenshotData;

        let rule = OrganizationRule {
            name: "DLSite".to_string(),
            is_enabled: true,
            trigger: RuleTrigger::default(),
            actions: RuleActions {
                root_folder: Some("[$product_id] $title".to_string()),
                use_standard_layout: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let meta = GameMetadata {
            product_id: "RJ123456".to_string(),
            source: "dlsite".to_string(),
            title: "Test".to_string(),
            description: None,
            tags: vec![],
            release_date: None,
            creator: None,
            screenshots: vec![
                ScreenshotData::FilePath("/imgs/main.jpg".into()),
                ScreenshotData::FilePath("/imgs/sub.jpg".into()),
            ],
            metadata_json: String::new(),
        };

        let plan =
            RuleEngine::create_plan(&rule, "RJ123456.zip", &[], Some(&meta))
                .expect("plan should succeed");

        assert_eq!(plan.downloads.len(), 2);
        assert_eq!(
            plan.downloads[0].cache_key,
            "dlsite:RJ123456:screenshot_0"
        );
        assert_eq!(
            plan.downloads[1].cache_key,
            "dlsite:RJ123456:screenshot_1"
        );
    }

    /// Non-dlsite sources use a different cache key prefix.
    #[test]
    fn test_screenshot_cache_keys_non_dlsite_source() {
        use crate::features::organization::metadata::ScreenshotData;

        let rule = OrganizationRule {
            name: "Other".to_string(),
            is_enabled: true,
            trigger: RuleTrigger::default(),
            actions: RuleActions {
                root_folder: Some("$product_id".to_string()),
                use_standard_layout: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let meta = GameMetadata {
            product_id: "12345".to_string(),
            source: "steam".to_string(),
            title: "Steam Game".to_string(),
            description: None,
            tags: vec![],
            release_date: None,
            creator: None,
            screenshots: vec![
                ScreenshotData::FilePath("/imgs/screen.png".into()),
            ],
            metadata_json: String::new(),
        };

        let plan =
            RuleEngine::create_plan(&rule, "game.zip", &[], Some(&meta))
                .expect("plan should succeed");

        assert_eq!(plan.downloads.len(), 1);
        assert_eq!(
            plan.downloads[0].cache_key,
            "screenshot:12345:0"
        );
    }
}
