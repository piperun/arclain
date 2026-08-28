//! `RuleEngine` impl — rule matching, plan generation, screenshot
//! download list, template-variable expansion, glob matching.
//!
//! Was the bulk of the old single-file `engine.rs`. Helpers like the
//! tree pruner live in [`super::tree`]; this file focuses on plan
//! construction. `expand_variables` and `matches_glob` are
//! `pub(super)` so the test suite in `mod.rs` can exercise them
//! directly without duplicating fixtures.

use super::tree::TreeNode;
use super::{OrganizationPlan, PendingDownload, RuleEngine};
use crate::features::organization::{OrganizationRule, RuleTrigger};
use crate::ArchiveEntry;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
        // Prune unnecessary files/folders before any analysis.
        let pruned_entries = Self::prune_entries(entries);
        let entries = &pruned_entries;

        // Detect the inner content root early — its folder name is one
        // of the metadata sources (version + tags), and the move
        // computation needs it too.
        let content_root = if rule.actions.use_standard_layout {
            Some(Self::find_game_content_root_in_entries(entries))
        } else {
            None
        };

        let metadata = Self::build_metadata_map(rule, archive_name, game_metadata, &content_root);

        let root_folder = rule
            .actions
            .root_folder
            .as_deref()
            .map(|tpl| Self::expand_variables(tpl, &metadata))
            .unwrap_or_else(|| "Game".to_string());

        let moves = Self::compute_moves(
            rule,
            entries,
            content_root.as_ref(),
            &metadata,
            &root_folder,
        );

        let mut generated_files = Vec::new();
        if let Some(gm) = game_metadata {
            // The layered document the plugin produced, which carries the
            // source-specific fields the extracted struct does not keep --
            // `metadata_json` is `#[serde(skip)]`, so serializing the struct
            // silently drops them. Fall back to the struct only when no raw
            // document came with the metadata.
            let contents = if gm.metadata_json.trim().is_empty() {
                serde_json::to_string_pretty(gm).ok()
            } else {
                Some(gm.metadata_json.clone())
            };
            if let Some(contents) = contents {
                generated_files.push((format!("{}/metadata.json", root_folder), contents));
            }
        }

        let downloads = Self::compute_downloads(rule, game_metadata, &root_folder);

        let root_folder_template = rule
            .actions
            .root_folder
            .clone()
            .unwrap_or_else(|| "Game".to_string());

        let plan = OrganizationPlan {
            rule_name: rule.name.clone(),
            root_folder,
            root_folder_template,
            moves,
            generated_files,
            downloads,
            use_standard_layout: rule.actions.use_standard_layout,
            resolved_variables: metadata,
        };
        plan.validate_paths()
            .context("organization plan path validation")?;
        Ok(plan)
    }

    /// Build the variable map used for expanding `$name` placeholders
    /// in the rule's `root_folder` and per-file move targets. Pulls
    /// from (in order, last write wins): GameMetadata fields,
    /// flattened `metadata_json`, named captures from
    /// `trigger.filename_pattern`, the archive filename version regex,
    /// and the inner content-root folder name (version + bracketed
    /// tags).
    fn build_metadata_map(
        rule: &OrganizationRule,
        archive_name: &str,
        game_metadata: Option<&crate::features::organization::metadata::GameMetadata>,
        content_root: &Option<PathBuf>,
    ) -> HashMap<String, String> {
        let mut metadata = HashMap::new();

        if let Some(gm) = game_metadata {
            metadata.insert("product_id".to_string(), gm.product_id.clone());
            metadata.insert("source".to_string(), gm.source.clone());
            metadata.insert("title".to_string(), gm.title.clone());

            // filtered_title is a folder-safe variant for templates
            // like `$creator/$filtered_title`.
            let filtered = crate::utilities::title_filter::sanitize_title(&gm.title);
            metadata.insert("filtered_title".to_string(), filtered);

            if let Some(creator) = &gm.creator {
                metadata.insert("creator".to_string(), creator.clone());
                metadata.insert("circle".to_string(), creator.clone()); // Alias
            }
            if let Some(date) = &gm.release_date {
                metadata.insert("release_date".to_string(), date.clone());
            }

            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&gm.metadata_json) {
                crate::features::organization::flatten_helper::flatten_json_value(
                    &json,
                    &mut metadata,
                    "",
                );
            }
        }

        // Filename-pattern named captures override/supplement
        // anything from GameMetadata.
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

        // Version from archive filename — `vN.M[.K]` — overrides
        // anything from JSON if present.
        if let Ok(re) = Regex::new(r"[vV](\d+(\.\d+)+)") {
            if let Some(caps) = re.captures(archive_name) {
                if let Some(v) = caps.get(1) {
                    metadata.insert("version".to_string(), v.as_str().to_string());
                }
            }
        }

        // Inner-folder version + tag extraction. Useful when the
        // archive contains a `Game_v1.0_[Patched]` wrapper.
        if let Some(root_path) = content_root {
            if let Some(folder_name) = root_path.file_name().and_then(|n| n.to_str()) {
                if let Ok(re) = Regex::new(r"[vV](\d+(\.\d+)+)") {
                    if let Some(caps) = re.captures(folder_name) {
                        if let Some(v) = caps.get(1) {
                            metadata.insert("version".to_string(), v.as_str().to_string());
                        }
                    }
                }

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

        metadata
    }

    /// Generate the (source_path, dest_path) move list. Three
    /// branches:
    ///
    /// * `content_root.is_some()` (sanitization mode) — flatten the
    ///   archive's wrapper folder, putting everything under
    ///   `{root_folder}/Game/...`.
    /// * `!use_standard_layout` (explicit-rule mode) — strip the
    ///   common parent path, then route each file through
    ///   `actions.move_files` glob rules into `{root_folder}/{target}/...`.
    /// * Otherwise — empty (caller has standard layout but no content
    ///   root was found).
    fn compute_moves(
        rule: &OrganizationRule,
        entries: &[ArchiveEntry],
        content_root: Option<&PathBuf>,
        metadata: &HashMap<String, String>,
        root_folder: &str,
    ) -> Vec<(String, String)> {
        let mut moves = Vec::new();

        if let Some(content_root) = content_root {
            let content_root_path = Path::new(content_root);

            for entry in entries {
                if entry.is_dir {
                    continue;
                }

                // Only include files inside the content root
                // (filters out junk wrappers efficiently).
                if let Ok(relative_content_path) =
                    Path::new(&entry.path).strip_prefix(content_root_path)
                {
                    let dest_path = format!(
                        "{}/Game/{}",
                        root_folder,
                        relative_content_path.to_string_lossy()
                    );
                    moves.push((entry.path.clone(), Self::normalize_dest(&dest_path)));
                }
            }
        } else if !rule.actions.use_standard_layout {
            let common_root = Self::common_parent(entries);

            for entry in entries {
                if entry.is_dir {
                    continue;
                }

                let mut target_dir = "game/".to_string(); // Default fallback
                for move_rule in &rule.actions.move_files {
                    if Self::matches_glob(&move_rule.pattern, &entry.path) {
                        target_dir = move_rule.target.clone();
                        break;
                    }
                }
                target_dir = Self::expand_variables(&target_dir, metadata);

                // Strip the common root so nested archives don't
                // double-up the wrapper folder; preserve the
                // remaining subdirectory structure.
                let relative_path = Path::new(&entry.path)
                    .strip_prefix(&common_root)
                    .unwrap_or(Path::new(&entry.path));

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
                moves.push((entry.path.clone(), Self::normalize_dest(&dest_path)));
            }
        }

        moves
    }

    /// Longest path prefix shared by every entry, used to strip the
    /// outer wrapper folder in explicit-rule mode.
    fn common_parent(entries: &[ArchiveEntry]) -> PathBuf {
        let paths: Vec<&Path> = entries.iter().map(|e| Path::new(&e.path)).collect();
        if paths.is_empty() {
            return PathBuf::new();
        }

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
    }

    /// Forward-slashify and collapse double slashes so plans built on
    /// Windows match the layout produced on Unix.
    fn normalize_dest(path: &str) -> String {
        path.replace("//", "/").replace('\\', "/")
    }

    /// Build the screenshot download list. Only URL screenshots are
    /// downloadable; a plugin that already fetched the file, or inlined
    /// the bytes, reports a form this plan cannot schedule and is
    /// skipped. DLsite uses cache keys keyed by product_id; other
    /// sources fall back to a generic `screenshot:` prefix.
    fn compute_downloads(
        rule: &OrganizationRule,
        game_metadata: Option<&crate::features::organization::metadata::GameMetadata>,
        root_folder: &str,
    ) -> Vec<PendingDownload> {
        let mut downloads = Vec::new();

        let Some(gm) = game_metadata else {
            return downloads;
        };
        let is_dlsite = gm.source.eq_ignore_ascii_case("dlsite");
        let screenshots_folder = if rule.actions.use_standard_layout {
            "screenshots"
        } else {
            "Screenshots"
        };

        for (i, screenshot) in gm.screenshots.iter().enumerate() {
            let crate::features::organization::metadata::ScreenshotData::Url(url) = screenshot
            else {
                continue;
            };

            let url = url.clone();
            let ext = Path::new(&url)
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_else(|| "jpg".to_string());

            let filename = format!("image_{:03}.{}", i + 1, ext);
            let dest_path = format!("{}/{}/{}", root_folder, screenshots_folder, filename);

            // Cache key must match gameta's cache_keys format.
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

        downloads
    }

    pub(super) fn expand_variables(template: &str, metadata: &HashMap<String, String>) -> String {
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

    pub(super) fn matches_glob(pattern: &str, path: &str) -> bool {
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

        // `www`, `data` and `js` are folder names, and a folder never
        // reaches this function as an entry: `create_plan` scores the
        // pruned list, and `TreeNode::flatten` keeps files only. Derive
        // the folders the file paths imply and score them by name, once
        // each -- crediting per file would let a folder holding five
        // hundred files outscore the layout it sits in.
        let mut implied_dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for entry in entries {
            let mut cursor = Path::new(&entry.path).parent();
            while let Some(dir) = cursor {
                if dir.as_os_str().is_empty() {
                    break;
                }
                implied_dirs.insert(dir.to_path_buf());
                cursor = dir.parent();
            }
        }
        for dir in &implied_dirs {
            let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
                continue;
            };
            let name = name.to_string_lossy();
            if game_indicators
                .iter()
                .any(|indicator| name.eq_ignore_ascii_case(indicator))
            {
                *dirs.entry(parent.to_path_buf()).or_insert(0) += 1;
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
