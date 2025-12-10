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
    /// Resolved template variables for UI display (e.g., "code" -> "RJ999001")
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
        game_metadata: Option<&super::organizer::GameMetadata>,
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
        game_metadata: Option<&super::organizer::GameMetadata>,
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
        game_metadata: Option<&super::organizer::GameMetadata>,
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
                Self::flatten_json_value(&json, &mut metadata, "");
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
            // Get DLsite code from metadata if available
            let dlsite_code = metadata.get("code").cloned();

            for (i, screenshot) in gm.screenshots.iter().enumerate() {
                if let super::organizer::ScreenshotData::FilePath(path) = screenshot {
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

                    // Generate cache key
                    let cache_key = if let Some(ref code) = dlsite_code {
                        format!("dlsite:{}:screenshot_{}", code, i)
                    } else {
                        format!("screenshot:{}:{}", gm.product_id, i)
                    };

                    downloads.push(PendingDownload {
                        product_id: dlsite_code.clone(),
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

    /// Recursive JSON flattener to extract variables like "dlsite.price" -> "1200"
    fn flatten_json_value(
        value: &serde_json::Value,
        acc: &mut HashMap<String, String>,
        prefix: &str,
    ) {
        match value {
            serde_json::Value::Null => {}
            serde_json::Value::Bool(b) => {
                acc.insert(prefix.to_string(), b.to_string());
            }
            serde_json::Value::Number(n) => {
                acc.insert(prefix.to_string(), n.to_string());
            }
            serde_json::Value::String(s) => {
                acc.insert(prefix.to_string(), s.clone());
            }
            serde_json::Value::Array(_) => {
                // Skip arrays for simple variable resolution for now
            }
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    let new_prefix = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{}.{}", prefix, k)
                    };
                    Self::flatten_json_value(v, acc, &new_prefix);
                }
            }
        }
    }
    /// Helper to find the "game content" root folder in entries
    /// Mimics logic from organizer.rs::find_and_flatten_game_content
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

        // Group entries by parent directory
        let mut dirs: HashMap<PathBuf, usize> = HashMap::new();

        for entry in entries {
            let path = Path::new(&entry.path);
            if let Some(parent) = path.parent() {
                // If this file is an indicator, score the parent
                if let Some(fname) = path.file_name() {
                    let fname_str = fname.to_string_lossy();
                    if game_indicators
                        .iter()
                        .any(|i| fname_str.eq_ignore_ascii_case(i))
                    {
                        *dirs.entry(parent.to_path_buf()).or_insert(0) += 1;
                    }
                }
            }
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
