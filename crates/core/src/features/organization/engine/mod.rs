//! Organization rule engine.
//!
//! Was a single 1077-LOC `engine.rs`. Split here so the file you open
//! tells you what's inside:
//!
//! - This file: shared types ([`PendingDownload`], [`OrganizationPlan`],
//!   the [`RuleEngine`] marker struct) and the test suite.
//! - [`plan_builder`] — the `impl RuleEngine` block (rule matching,
//!   plan generation, screenshot download list, glob/template helpers).
//! - [`tree`] — the `TreeNode` path tree used by `prune_entries` and
//!   `find_game_content_root_in_entries`.

mod plan_builder;
mod tree;

use std::collections::HashMap;

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
    /// Set by the rule. Nothing branches on it: the applier works from the
    /// move list, so the flag survives only as data carried to the preview.
    /// Kept because the `RuleActions::use_standard_layout` rule action it
    /// mirrors is still live; both retire when layouts stop being a boolean.
    pub use_standard_layout: bool,
    /// Resolved template variables for UI display (e.g., "code" -> "RJ123456")
    pub resolved_variables: HashMap<String, String>,
}

impl OrganizationPlan {
    /// Validate every filesystem path carried by this plan before it reaches
    /// an execution boundary.
    pub fn validate_paths(&self) -> anyhow::Result<()> {
        use anyhow::Context;
        use std::collections::HashSet;

        crate::utilities::CheckedRelativePath::new(&self.root_folder)
            .context("invalid organization root_folder")?;

        let collision_key = |path: &crate::utilities::CheckedRelativePath| {
            path.as_path()
                .to_string_lossy()
                .replace('\\', "/")
                .to_uppercase()
        };
        let mut destinations = HashSet::new();
        for (source, destination) in &self.moves {
            crate::utilities::CheckedRelativePath::new(source)
                .with_context(|| format!("invalid move source {source:?}"))?;
            let destination = crate::utilities::CheckedRelativePath::new(destination)
                .with_context(|| format!("invalid move destination {destination:?}"))?;
            if !destinations.insert(collision_key(&destination)) {
                anyhow::bail!(
                    "duplicate organization destination {:?}",
                    destination.as_path()
                );
            }
        }

        for (path, _) in &self.generated_files {
            let path = crate::utilities::CheckedRelativePath::new(path)
                .with_context(|| format!("invalid generated file path {path:?}"))?;
            if !destinations.insert(collision_key(&path)) {
                anyhow::bail!("duplicate organization destination {:?}", path.as_path());
            }
        }

        for download in &self.downloads {
            let destination = crate::utilities::CheckedRelativePath::new(&download.dest_path)
                .with_context(|| format!("invalid download path {:?}", download.dest_path))?;
            if !destinations.insert(collision_key(&destination)) {
                anyhow::bail!(
                    "duplicate organization destination {:?}",
                    destination.as_path()
                );
            }
        }

        Ok(())
    }
}

pub struct RuleEngine;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::organization::metadata::GameMetadata;
    use crate::features::organization::{OrganizationRule, RuleActions, RuleTrigger};
    use crate::ArchiveEntry;

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
        assert!(RuleEngine::matches_trigger(
            &trigger,
            "anything.7z",
            &[],
            None
        ));
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
            &trigger, "test.7z", &entries, None
        ));

        let no_match = vec![make_entry("folder/readme.txt", 100, false)];
        assert!(!RuleEngine::matches_trigger(
            &trigger, "test.7z", &no_match, None
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
        assert!(!RuleEngine::matches_trigger(&trigger, "test.7z", &[], None));
    }

    #[test]
    fn test_matches_trigger_invalid_regex_returns_false() {
        let trigger = RuleTrigger {
            filename_pattern: Some("[invalid".to_string()),
            ..Default::default()
        };
        assert!(!RuleEngine::matches_trigger(&trigger, "test.7z", &[], None));
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
        assert_eq!(RuleEngine::expand_variables("$unknown", &vars), "$unknown");
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

    #[test]
    fn create_plan_rejects_escaping_root_folder() {
        let rule = OrganizationRule {
            name: "unsafe".into(),
            actions: RuleActions {
                root_folder: Some("../outside".into()),
                use_standard_layout: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let error = RuleEngine::create_plan(&rule, "game.zip", &[], None).unwrap_err();
        let error = format!("{error:#}");

        assert!(error.contains("root_folder"), "unexpected error: {error}");
    }

    #[test]
    fn validate_paths_rejects_case_insensitive_cross_category_collision() {
        let plan = OrganizationPlan {
            rule_name: "unsafe".into(),
            root_folder: "MyGame".into(),
            root_folder_template: "MyGame".into(),
            moves: vec![("game.exe".into(), "MyGame/Output.bin".into())],
            generated_files: vec![("mygame/output.BIN".into(), "generated".into())],
            downloads: vec![],
            use_standard_layout: false,
            resolved_variables: Default::default(),
        };

        assert!(plan.validate_paths().is_err());
    }

    #[test]
    fn validate_paths_rejects_case_insensitive_download_collision() {
        let plan = OrganizationPlan {
            rule_name: "unsafe".into(),
            root_folder: "MyGame".into(),
            root_folder_template: "MyGame".into(),
            moves: vec![("game.exe".into(), "MyGame/Cover.JPG".into())],
            generated_files: vec![],
            downloads: vec![PendingDownload {
                product_id: None,
                url: "https://example.invalid/cover.jpg".into(),
                dest_path: "mygame/cover.jpg".into(),
                cache_key: "cover".into(),
                cached: false,
            }],
            use_standard_layout: false,
            resolved_variables: Default::default(),
        };

        assert!(plan.validate_paths().is_err());
    }

    #[test]
    fn validate_paths_rejects_unicode_sigma_collision() {
        let plan = OrganizationPlan {
            rule_name: "unsafe".into(),
            root_folder: "MyGame".into(),
            root_folder_template: "MyGame".into(),
            moves: vec![("game.exe".into(), "MyGame/σ.bin".into())],
            generated_files: vec![("mygame/ς.BIN".into(), "generated".into())],
            downloads: vec![],
            use_standard_layout: false,
            resolved_variables: Default::default(),
        };

        assert!(plan.validate_paths().is_err());
    }

    #[test]
    fn validate_paths_rejects_uppercase_expansion_collision() {
        let plan = OrganizationPlan {
            rule_name: "unsafe".into(),
            root_folder: "MyGame".into(),
            root_folder_template: "MyGame".into(),
            moves: vec![("game.exe".into(), "MyGame/straße.bin".into())],
            generated_files: vec![],
            downloads: vec![PendingDownload {
                product_id: None,
                url: "https://example.invalid/output".into(),
                dest_path: "mygame/STRASSE.BIN".into(),
                cache_key: "output".into(),
                cached: false,
            }],
            use_standard_layout: false,
            resolved_variables: Default::default(),
        };

        assert!(plan.validate_paths().is_err());
    }

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
                ScreenshotData::Url("https://img.example.test/main.jpg".to_string()),
                ScreenshotData::Url("https://img.example.test/sub.jpg".to_string()),
            ],
            metadata_json: String::new(),
        };

        let plan = RuleEngine::create_plan(&rule, "RJ123456.zip", &[], Some(&meta))
            .expect("plan should succeed");

        assert_eq!(plan.downloads.len(), 2);
        assert_eq!(plan.downloads[0].cache_key, "dlsite:RJ123456:screenshot_0");
        assert_eq!(plan.downloads[1].cache_key, "dlsite:RJ123456:screenshot_1");
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
            screenshots: vec![ScreenshotData::Url(
                "https://img.example.test/screen.png".to_string(),
            )],
            metadata_json: String::new(),
        };

        let plan = RuleEngine::create_plan(&rule, "game.zip", &[], Some(&meta))
            .expect("plan should succeed");

        assert_eq!(plan.downloads.len(), 1);
        assert_eq!(plan.downloads[0].cache_key, "screenshot:12345:0");
    }

    // =========================================================================
    // generated metadata.json
    // =========================================================================

    fn placeholder_metadata(metadata_json: &str) -> GameMetadata {
        GameMetadata {
            product_id: "RJ123456".to_string(),
            source: "dlsite".to_string(),
            title: "Placeholder Game".to_string(),
            description: None,
            tags: vec![],
            release_date: None,
            creator: Some("Placeholder Circle".to_string()),
            screenshots: vec![],
            metadata_json: metadata_json.to_string(),
        }
    }

    fn metadata_rule() -> OrganizationRule {
        OrganizationRule {
            name: "Standard".to_string(),
            is_enabled: true,
            trigger: RuleTrigger::default(),
            actions: RuleActions {
                root_folder: Some("$product_id".to_string()),
                use_standard_layout: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn generated_metadata_json(plan: &OrganizationPlan) -> &str {
        plan.generated_files
            .iter()
            .find(|(path, _)| path == "RJ123456/metadata.json")
            .map(|(_, contents)| contents.as_str())
            .expect("plan should generate metadata.json")
    }

    /// The layered document a plugin produced is written through
    /// unchanged. Serializing `GameMetadata` instead would drop every
    /// source-specific field, because `metadata_json` is
    /// `#[serde(skip)]` and the nested object lives only in there.
    #[test]
    fn create_plan_writes_the_layered_metadata_document_verbatim() {
        let layered = r#"{
  "product_id": "RJ123456",
  "title": "Placeholder Game",
  "dlsite": {
    "circle": "Placeholder Circle",
    "work_format": "Placeholder Format"
  }
}"#;
        let meta = placeholder_metadata(layered);

        let plan = RuleEngine::create_plan(&metadata_rule(), "RJ123456.zip", &[], Some(&meta))
            .expect("plan should succeed");
        let contents = generated_metadata_json(&plan);

        assert_eq!(contents, layered);
        assert!(
            contents.contains("work_format"),
            "the source-specific fields the struct does not carry were dropped: {contents}"
        );
    }

    /// Metadata that arrived without a raw document still gets a
    /// metadata.json: the extracted struct is the fallback.
    #[test]
    fn create_plan_falls_back_to_the_struct_when_no_document_came_with_the_metadata() {
        for empty in ["", "   \n"] {
            let meta = placeholder_metadata(empty);

            let plan = RuleEngine::create_plan(&metadata_rule(), "RJ123456.zip", &[], Some(&meta))
                .expect("plan should succeed");
            let contents = generated_metadata_json(&plan);

            let parsed: serde_json::Value =
                serde_json::from_str(contents).expect("fallback metadata.json should parse");
            assert_eq!(parsed["product_id"], "RJ123456");
            assert_eq!(parsed["title"], "Placeholder Game");
        }
    }
    /// Three of the content-root indicators are folder names, and a
    /// folder never reaches the scorer: `create_plan` scores the pruned
    /// list, and `TreeNode::flatten` keeps files only. So a layout whose
    /// only signal is its folder names scored below the threshold and
    /// fell back to the common prefix, dragging the wrapper along.
    #[test]
    fn folder_name_indicators_locate_the_content_root() {
        use crate::archive::ArchiveEntry;

        fn entry(path: &str) -> ArchiveEntry {
            ArchiveEntry {
                path: path.to_string(),
                size: 10,
                packed_size: 10,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            }
        }

        let entries = vec![
            entry("Placeholder Wrapper/www/index.html"),
            entry("Placeholder Wrapper/www/js/main.js"),
            entry("Placeholder Wrapper/www/data/System.json"),
            entry("Placeholder Wrapper/readme.txt"),
        ];

        let rule = OrganizationRule {
            name: "Standard".to_string(),
            is_enabled: true,
            trigger: RuleTrigger::default(),
            actions: RuleActions {
                root_folder: Some("Out".to_string()),
                use_standard_layout: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let plan = RuleEngine::create_plan(&rule, "placeholder.zip", &entries, None)
            .expect("plan should build");
        let destinations: std::collections::BTreeSet<_> =
            plan.moves.iter().map(|(_, to)| to.clone()).collect();

        assert!(
            destinations.contains("Out/Game/index.html"),
            "the www folder is the content root, so its contents sit directly under Game/: {destinations:?}"
        );
        assert!(
            destinations.contains("Out/Game/js/main.js"),
            "subdirectories of the content root keep their structure: {destinations:?}"
        );
        assert!(
            !destinations.iter().any(|d| d.contains("/www/")),
            "the wrapper and the www folder itself must not survive into the layout: {destinations:?}"
        );
    }
}
