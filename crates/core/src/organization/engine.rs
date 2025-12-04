use super::{OrganizationRule, RuleTrigger};
use crate::ArchiveEntry;
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct OrganizationPlan {
    pub rule_name: String,
    pub root_folder: String,
    pub moves: Vec<(String, String)>, // (source_path, dest_path)
    pub generated_files: Vec<(String, String)>, // (path, content)
    pub downloads: Vec<(String, String)>, // (url, relative_path)
}

pub struct RuleEngine;

impl RuleEngine {
    /// Find all rules that match the given archive
    pub fn find_matching_rules(
        rules: &[OrganizationRule],
        archive_name: &str,
        entries: &[ArchiveEntry],
    ) -> Vec<OrganizationRule> {
        let mut matches = Vec::new();

        for rule in rules {
            if !rule.is_enabled {
                continue;
            }

            if Self::matches_trigger(&rule.trigger, archive_name, entries) {
                matches.push(rule.clone());
            }
        }

        // Sort by priority (descending)
        matches.sort_by(|a, b| b.priority.cmp(&a.priority));
        matches
    }

    fn matches_trigger(
        trigger: &RuleTrigger,
        archive_name: &str,
        entries: &[ArchiveEntry],
    ) -> bool {
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

            // Parse JSON for platform-specific fields (e.g. dlsite.price)
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&gm.metadata_json) {
                if let Some(dlsite) = json.get("dlsite") {
                    if let Some(price) = dlsite.get("price").and_then(|v| v.as_str()) {
                        metadata.insert("price".to_string(), price.to_string());
                    }
                    if let Some(code) = dlsite.get("code").and_then(|v| v.as_str()) {
                        metadata.insert("code".to_string(), code.to_string());
                    }
                }
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

        // Determine root folder name
        let root_folder = if let Some(root_template) = &rule.actions.root_folder {
            Self::expand_variables(root_template, &metadata)
        } else {
            // Default to archive name without extension if not specified?
            // Or just "Game"?
            "Game".to_string()
        };

        // Process file moves
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

            // Construct new path
            // e.g. root_folder/target_dir/filename
            let filename = Path::new(&entry.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();

            let new_path = format!("{}/{}/{}", root_folder, target_dir, filename)
                .replace("//", "/")
                .replace("\\", "/");

            moves.push((entry.path.clone(), new_path));
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
            for (i, screenshot) in gm.screenshots.iter().enumerate() {
                if let super::organizer::ScreenshotData::FilePath(path) = screenshot {
                    let url = path.to_string_lossy().to_string();
                    // Determine extension from URL or default to jpg
                    let ext = Path::new(&url)
                        .extension()
                        .map(|e| e.to_string_lossy().to_string())
                        .unwrap_or_else(|| "jpg".to_string());

                    let filename = format!("image_{:03}.{}", i + 1, ext);
                    let target_path = format!("{}/Screenshots/{}", root_folder, filename);
                    downloads.push((url, target_path));
                }
            }
        }

        Ok(OrganizationPlan {
            rule_name: rule.name.clone(),
            root_folder,
            moves,
            generated_files,
            downloads,
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
