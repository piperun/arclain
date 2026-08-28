//! Organization rule engine.
//!
//! Was a single 1077-LOC `engine.rs`. Split here so the file you open
//! tells you what's inside:
//!
//! - This file: shared types ([`PendingDownload`], [`OrganizationPlan`],
//!   the [`RuleEngine`] marker struct) and the test suite.
//! - [`outputs`] — resolving a `Layout` to its named outputs: how many
//!   there are, where each one's content starts, what each is called.
//! - [`plan_builder`] — the `impl RuleEngine` block (rule matching,
//!   plan assembly, filling one output from its layout, glob helpers).
//! - [`tree`] — the `TreeNode` path tree used by `prune_entries` and
//!   `find_game_content_root_in_entries`.

mod outputs;
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

/// One output of an organized archive, filled in: every file that goes
/// into it, every file written into it, and every image fetched into it.
///
/// An archive can produce several of these — a mod pack is one folder
/// per mod — so the paths here are complete destinations rather than
/// anything relative to a plan-wide root.
#[derive(Debug, Clone)]
pub struct PlannedOutput {
    /// The output's folder name, resolved. Empty means no wrapper.
    pub root_folder: String,
    /// The template `root_folder` came from, kept so a preview can show
    /// what was asked for next to what it turned into.
    pub root_folder_template: String,
    /// `(source_path, dest_path)` per file carried into this output.
    pub moves: Vec<(String, String)>,
    /// `(path, content)` per file written into this output.
    pub generated_files: Vec<(String, String)>,
    pub downloads: Vec<PendingDownload>,
    /// The variables this output's templates were expanded against.
    pub resolved_variables: HashMap<String, String>,
    /// Why this output looks the way it does, for the preview to show.
    /// A verdict without its evidence is a thing a user can only trust
    /// or distrust; with it they can check.
    pub reasoning: Vec<String>,
}

/// Everything one run of a rule will produce.
///
/// An archive is not one output: a mod pack is one folder per mod. The
/// per-output fields therefore live on [`PlannedOutput`] and the plan
/// itself carries only what is true of the whole run.
#[derive(Debug, Clone)]
pub struct OrganizationPlan {
    pub rule_name: String,
    pub outputs: Vec<PlannedOutput>,
    /// One `(root, reason)` per output that could not be named, so a
    /// caller can say which folder was passed over and why. The same
    /// shape `StagedPlan::unfetched` uses, for the same reason: a thing
    /// the run skipped is not an error, but it must not be silent.
    pub skipped_outputs: Vec<(String, String)>,
}

impl OrganizationPlan {
    /// Validate every filesystem path carried by this plan before it reaches
    /// an execution boundary.
    ///
    /// Destinations are pooled across outputs rather than checked one
    /// output at a time. Two outputs cannot share a root name —
    /// resolution refuses that — but pooling costs nothing and means a
    /// layout that reaches past its own root still cannot land two files
    /// on one path.
    pub fn validate_paths(&self) -> anyhow::Result<()> {
        use anyhow::Context;
        use std::collections::HashSet;

        let collision_key = |path: &crate::utilities::CheckedRelativePath| {
            path.as_path()
                .to_string_lossy()
                .replace('\\', "/")
                .to_uppercase()
        };
        let mut destinations = HashSet::new();

        for output in &self.outputs {
            // An empty root folder means the output has no wrapper and
            // its content sits at the top level, which resolution only
            // permits for a lone output. There is no path to check.
            if !output.root_folder.is_empty() {
                crate::utilities::CheckedRelativePath::new(&output.root_folder)
                    .context("invalid organization root_folder")?;
            }

            for (source, destination) in &output.moves {
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

            for (path, _) in &output.generated_files {
                let path = crate::utilities::CheckedRelativePath::new(path)
                    .with_context(|| format!("invalid generated file path {path:?}"))?;
                if !destinations.insert(collision_key(&path)) {
                    anyhow::bail!("duplicate organization destination {:?}", path.as_path());
                }
            }

            for download in &output.downloads {
                let destination = crate::utilities::CheckedRelativePath::new(&download.dest_path)
                    .with_context(|| {
                    format!("invalid download path {:?}", download.dest_path)
                })?;
                if !destinations.insert(collision_key(&destination)) {
                    anyhow::bail!(
                        "duplicate organization destination {:?}",
                        destination.as_path()
                    );
                }
            }
        }

        Ok(())
    }
}

pub struct RuleEngine;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::organization::layout::{
        FetchSource, Fetched, Generated, GeneratedContent, Layout, OutputSelector, Placement,
        Source,
    };
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

    /// No layout below declares a file variable, so nothing here reads a
    /// byte out of the input.
    fn no_reads(_: &str) -> Option<Vec<u8>> {
        None
    }

    /// The layout the retired `use_standard_layout: true` stood for,
    /// written out rather than translated. A test that means "the
    /// standard shape" must not pass merely because the translation
    /// agrees with itself.
    fn standard_layout(name: &str) -> Layout {
        Layout {
            outputs: OutputSelector::Whole,
            file_variables: vec![],
            name: name.to_string(),
            place: vec![Placement {
                from: Source::ContentRoot,
                into: "Game".to_string(),
            }],
            generate: vec![Generated {
                into: "metadata.json".to_string(),
                content: GeneratedContent::MetadataDocument,
            }],
            fetch: vec![Fetched {
                into: "screenshots".to_string(),
                source: FetchSource::Screenshots,
                name: "image_$index.$ext".to_string(),
            }],
        }
    }

    fn rule_named(name: &str, layout: Layout) -> OrganizationRule {
        OrganizationRule {
            name: name.to_string(),
            is_enabled: true,
            trigger: RuleTrigger::default(),
            actions: RuleActions {
                output_name: None,
                layout,
            },
            ..Default::default()
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

    /// `dir/**` is how a placement names a folder. Case is folded the
    /// way the `*.ext` branch already folds it, so one glob does not
    /// answer two ways depending on how an archive happened to spell a
    /// folder; and the folder's own name is not a file under it.
    #[test]
    fn test_matches_glob_names_a_folder() {
        assert!(RuleEngine::matches_glob("docs/**", "docs/manual.pdf"));
        assert!(RuleEngine::matches_glob("docs/**", "docs/deep/manual.pdf"));
        assert!(RuleEngine::matches_glob("docs/**", "Docs/manual.pdf"));
        assert!(!RuleEngine::matches_glob("docs/**", "docs"));
        assert!(!RuleEngine::matches_glob("docs/**", "documents/manual.pdf"));
        assert!(!RuleEngine::matches_glob(
            "docs/**",
            "other/docs/manual.pdf"
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
        let rule = rule_named(
            "unsafe",
            Layout {
                name: "../outside".to_string(),
                ..Layout::default()
            },
        );

        let error = RuleEngine::create_plan(&rule, "game.zip", &[], None, &no_reads).unwrap_err();
        let error = format!("{error:#}");

        assert!(error.contains("root_folder"), "unexpected error: {error}");
    }

    /// A plan carrying one output, for the validation tests. They are
    /// about `validate_paths` refusing destination pairs a filesystem
    /// would merge, not about how an output came to hold them.
    fn plan_with(output: PlannedOutput) -> OrganizationPlan {
        OrganizationPlan {
            rule_name: "unsafe".into(),
            outputs: vec![output],
            skipped_outputs: vec![],
        }
    }

    fn output_holding(
        moves: Vec<(String, String)>,
        generated_files: Vec<(String, String)>,
        downloads: Vec<PendingDownload>,
    ) -> PlannedOutput {
        PlannedOutput {
            root_folder: "MyGame".into(),
            root_folder_template: "MyGame".into(),
            moves,
            generated_files,
            downloads,
            resolved_variables: Default::default(),
            reasoning: vec![],
        }
    }

    #[test]
    fn validate_paths_rejects_case_insensitive_cross_category_collision() {
        let plan = plan_with(output_holding(
            vec![("game.exe".into(), "MyGame/Output.bin".into())],
            vec![("mygame/output.BIN".into(), "generated".into())],
            vec![],
        ));

        assert!(plan.validate_paths().is_err());
    }

    #[test]
    fn validate_paths_rejects_case_insensitive_download_collision() {
        let plan = plan_with(output_holding(
            vec![("game.exe".into(), "MyGame/Cover.JPG".into())],
            vec![],
            vec![PendingDownload {
                product_id: None,
                url: "https://example.invalid/cover.jpg".into(),
                dest_path: "mygame/cover.jpg".into(),
                cache_key: "cover".into(),
                cached: false,
            }],
        ));

        assert!(plan.validate_paths().is_err());
    }

    #[test]
    fn validate_paths_rejects_unicode_sigma_collision() {
        let plan = plan_with(output_holding(
            vec![("game.exe".into(), "MyGame/σ.bin".into())],
            vec![("mygame/ς.BIN".into(), "generated".into())],
            vec![],
        ));

        assert!(plan.validate_paths().is_err());
    }

    #[test]
    fn validate_paths_rejects_uppercase_expansion_collision() {
        let plan = plan_with(output_holding(
            vec![("game.exe".into(), "MyGame/straße.bin".into())],
            vec![],
            vec![PendingDownload {
                product_id: None,
                url: "https://example.invalid/output".into(),
                dest_path: "mygame/STRASSE.BIN".into(),
                cache_key: "output".into(),
                cached: false,
            }],
        ));

        assert!(plan.validate_paths().is_err());
    }

    /// One output's destinations must not be checked in isolation from
    /// its siblings'. A layout whose `into` reaches out of its own root
    /// can put two outputs on one path, and each output alone looks
    /// fine.
    #[test]
    fn validate_paths_rejects_a_collision_between_two_outputs() {
        let mut first = output_holding(
            vec![("a/game.exe".into(), "shared/Game.exe".into())],
            vec![],
            vec![],
        );
        first.root_folder = "First".into();
        let mut second = output_holding(
            vec![("b/game.exe".into(), "shared/Game.exe".into())],
            vec![],
            vec![],
        );
        second.root_folder = "Second".into();

        let plan = OrganizationPlan {
            rule_name: "unsafe".into(),
            outputs: vec![first, second],
            skipped_outputs: vec![],
        };

        assert!(plan.validate_paths().is_err());
    }

    /// An empty root folder means the output has no wrapper and its
    /// content sits at the top level. That is a legal layout, and the
    /// path check must not read it as an empty path.
    #[test]
    fn validate_paths_accepts_an_output_with_no_wrapper() {
        let mut output = output_holding(
            vec![("wrapper/Game.exe".into(), "Game.exe".into())],
            vec![],
            vec![],
        );
        output.root_folder = String::new();
        output.root_folder_template = String::new();

        plan_with(output)
            .validate_paths()
            .expect("no wrapper is legal");
    }

    /// Regression: screenshot cache keys must use `gm.product_id` directly
    /// so they match the keys produced by the gameta server cache.
    /// Previously the code looked up a "code" key in the metadata hashmap,
    /// which could differ from the actual product_id.
    #[test]
    fn test_screenshot_cache_keys_use_product_id_for_dlsite() {
        use crate::features::organization::metadata::ScreenshotData;

        let rule = rule_named("DLSite", standard_layout("[$product_id] $title"));

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

        let plan = RuleEngine::create_plan(&rule, "RJ123456.zip", &[], Some(&meta), &no_reads)
            .expect("plan should succeed");

        let downloads = &plan.outputs[0].downloads;
        assert_eq!(downloads.len(), 2);
        assert_eq!(downloads[0].cache_key, "dlsite:RJ123456:screenshot_0");
        assert_eq!(downloads[1].cache_key, "dlsite:RJ123456:screenshot_1");
    }

    /// Non-dlsite sources use a different cache key prefix.
    #[test]
    fn test_screenshot_cache_keys_non_dlsite_source() {
        use crate::features::organization::metadata::ScreenshotData;

        let rule = rule_named("Other", standard_layout("$product_id"));

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

        let plan = RuleEngine::create_plan(&rule, "game.zip", &[], Some(&meta), &no_reads)
            .expect("plan should succeed");

        let downloads = &plan.outputs[0].downloads;
        assert_eq!(downloads.len(), 1);
        assert_eq!(downloads[0].cache_key, "screenshot:12345:0");
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
        rule_named("Standard", standard_layout("$product_id"))
    }

    fn generated_metadata_json(plan: &OrganizationPlan) -> &str {
        plan.outputs[0]
            .generated_files
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

        let plan = RuleEngine::create_plan(
            &metadata_rule(),
            "RJ123456.zip",
            &[],
            Some(&meta),
            &no_reads,
        )
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

            let plan = RuleEngine::create_plan(
                &metadata_rule(),
                "RJ123456.zip",
                &[],
                Some(&meta),
                &no_reads,
            )
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

        let rule = rule_named("Standard", standard_layout("Out"));

        let plan = RuleEngine::create_plan(&rule, "placeholder.zip", &entries, None, &no_reads)
            .expect("plan should build");
        let destinations: std::collections::BTreeSet<_> = plan.outputs[0]
            .moves
            .iter()
            .map(|(_, to)| to.clone())
            .collect();

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

    /// Two folders scoring identically must resolve the same way on
    /// every run. The scorer iterates a HashMap, so before the tie-break
    /// this passed or failed depending on hash order.
    #[test]
    fn a_tied_content_root_score_resolves_to_the_same_folder_every_run() {
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

        // Both candidates score 2: an index.html and a js/ folder each.
        let entries = vec![
            entry("alpha/index.html"),
            entry("alpha/js/main.js"),
            entry("beta/index.html"),
            entry("beta/js/main.js"),
        ];

        let rule = rule_named("Standard", standard_layout("Out"));

        let first = RuleEngine::create_plan(&rule, "placeholder.zip", &entries, None, &no_reads)
            .expect("plan");
        for _ in 0..20 {
            let again =
                RuleEngine::create_plan(&rule, "placeholder.zip", &entries, None, &no_reads)
                    .expect("plan");
            assert_eq!(
                again.outputs[0].moves, first.outputs[0].moves,
                "a tie must not depend on hash order"
            );
        }
        assert!(
            first.outputs[0]
                .moves
                .iter()
                .any(|(from, _)| from.starts_with("alpha/")),
            "the lexicographically first candidate wins a tie: {:?}",
            first.outputs[0].moves
        );
    }

    // =========================================================================
    // translation of rules stored under the old vocabulary
    // =========================================================================

    /// The product rule as a user's database holds it: the exact JSON
    /// `config::defaults::get_default_rules()` serialized to before this
    /// change, captured by serializing it rather than written by hand,
    /// so the fixture is the real stored shape and not an approximation
    /// of it.
    const SHIPPED_PRODUCT_RULE_JSON: &str = r#"{"id":0,"name":"DLSite Archive","priority":100,"is_enabled":true,"trigger":{"metadata_source":"dlsite","filename_pattern":"\\[(RJ|BJ|VJ)\\d+\\]","has_file":null},"actions":{"root_folder":"[$product_id][$circle] $title","output_name":null,"move_files":[],"use_standard_layout":true}}"#;

    /// The tree the engine built from that stored rule before layouts
    /// were data, captured from a run of the retired code path. Every
    /// assertion below is one line of that capture.
    const SHIPPED_PRODUCT_ROOT: &str = "[RJ123456][Placeholder Circle] Placeholder Game";

    /// Translation is the promise that nobody's library reorganizes
    /// differently. Plan the shipped product rule — still stored in the
    /// old vocabulary — and require the tree the old code path built.
    ///
    /// Download destinations are asserted, not only moves and generated
    /// files. A translation that dropped `$index`'s zero-padding, or
    /// spelled `.jpg` into the template instead of taking the extension
    /// from the source URL, produces exactly the same moves and exactly
    /// the same metadata.json; the screenshot names are the only place
    /// it shows.
    #[test]
    fn the_shipped_product_rule_translates_to_an_identical_plan() {
        use crate::archive::ArchiveEntry;
        use crate::features::organization::metadata::ScreenshotData;

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
            entry("RJ123456/[v1.2] Placeholder Game/Game.exe"),
            entry("RJ123456/[v1.2] Placeholder Game/data/pack.bin"),
            entry("RJ123456/readme.txt"),
        ];
        let metadata = GameMetadata {
            product_id: "RJ123456".to_string(),
            source: "dlsite".to_string(),
            title: "Placeholder Game".to_string(),
            description: None,
            tags: vec![],
            release_date: None,
            creator: Some("Placeholder Circle".to_string()),
            // One `.jpg` and one `.png`, because a template that spells
            // the extension out passes a single-format list.
            screenshots: vec![
                ScreenshotData::Url("https://img.example.test/main.jpg".to_string()),
                ScreenshotData::Url("https://img.example.test/sub.png".to_string()),
            ],
            metadata_json: "{}".to_string(),
        };

        // The rule as it is stored today, in the old vocabulary.
        let stored: OrganizationRule = serde_json::from_str(SHIPPED_PRODUCT_RULE_JSON)
            .expect("the shipped rule must still deserialize");

        let plan = RuleEngine::create_plan(
            &stored,
            "[RJ123456] Placeholder Game.zip",
            &entries,
            Some(&metadata),
            &no_reads,
        )
        .expect("plan");

        assert_eq!(plan.outputs.len(), 1, "a product layout is one output");
        assert!(
            plan.skipped_outputs.is_empty(),
            "nothing was skipped: {:?}",
            plan.skipped_outputs
        );
        let output = &plan.outputs[0];
        assert_eq!(output.root_folder, SHIPPED_PRODUCT_ROOT);
        assert_eq!(output.root_folder_template, "[$product_id][$circle] $title");

        let root = SHIPPED_PRODUCT_ROOT;
        let destinations: std::collections::BTreeSet<_> =
            output.moves.iter().map(|(_, to)| to.clone()).collect();
        assert!(destinations.contains(&format!("{root}/Game/Game.exe")));
        assert!(destinations.contains(&format!("{root}/Game/data/pack.bin")));
        assert!(
            !destinations.iter().any(|d| d.contains("readme.txt")),
            "a file outside the content root is not carried: {destinations:?}"
        );
        assert_eq!(destinations.len(), 2, "and nothing else: {destinations:?}");

        assert_eq!(
            output
                .generated_files
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            vec![format!("{root}/metadata.json")],
            "the metadata document is still generated, in the same place"
        );

        assert_eq!(
            output
                .downloads
                .iter()
                .map(|download| (download.dest_path.as_str(), download.cache_key.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    format!("{root}/screenshots/image_001.jpg").as_str(),
                    "dlsite:RJ123456:screenshot_0"
                ),
                (
                    format!("{root}/screenshots/image_002.png").as_str(),
                    "dlsite:RJ123456:screenshot_1"
                ),
            ],
            "screenshot names are zero-padded and take their extension from the source URL"
        );
    }

    /// A stored rule that never turned the boolean on routed its files
    /// through `move_files` and fetched into a differently capitalised
    /// folder. Both survive translation, capitalisation included: making
    /// the two agree would move an existing rule's files.
    ///
    /// Two things this rule shape used to do are not reproduced, because
    /// the layout vocabulary has no way to say either and inventing one
    /// would be a second answer to a question `ContentRoot` already
    /// answers:
    ///
    /// * The retired code stripped the entries' longest common path
    ///   prefix before appending the target, so `wrapper/Game.exe` with
    ///   a target of `bin` landed at `Out/bin/Game.exe`. A placement
    ///   strips only what its own glob spells out, so the same file now
    ///   lands at `Out/bin/wrapper/Game.exe` — asserted below, so the
    ///   difference is pinned rather than discovered. Stripping a
    ///   wrapper is what a `ContentRoot` placement is for, and it is
    ///   a property of the archive rather than of the layout, which is
    ///   why no field can carry it.
    /// * A file no pattern matched used to fall through to a `game/`
    ///   folder. Placements claim files and a file nothing claimed is
    ///   not carried, which the output's `reasoning` says in as many
    ///   words rather than leaving silent.
    #[test]
    fn a_stored_rule_without_the_standard_layout_keeps_its_own_screenshot_folder() {
        use crate::features::organization::metadata::ScreenshotData;

        let stored: OrganizationRule = serde_json::from_str(
            r#"{"id":0,"name":"Explicit","priority":1,"is_enabled":true,
                "trigger":{"metadata_source":null,"filename_pattern":null,"has_file":null},
                "actions":{"root_folder":"Out","output_name":null,
                    "move_files":[{"pattern":"*.exe","target":"bin"}],
                    "use_standard_layout":false}}"#,
        )
        .expect("an old explicit rule must still deserialize");

        let meta = GameMetadata {
            product_id: "RJ999001".to_string(),
            source: "dlsite".to_string(),
            title: "Placeholder Game".to_string(),
            description: None,
            tags: vec![],
            release_date: None,
            creator: None,
            screenshots: vec![ScreenshotData::Url(
                "https://img.example.test/main.jpg".to_string(),
            )],
            metadata_json: "{}".to_string(),
        };

        let entries = vec![
            make_entry("wrapper/Game.exe", 10, false),
            make_entry("wrapper/readme.txt", 10, false),
        ];
        let plan = RuleEngine::create_plan(&stored, "game.zip", &entries, Some(&meta), &no_reads)
            .expect("plan");

        let output = &plan.outputs[0];
        assert_eq!(output.root_folder, "Out");
        assert_eq!(
            output.moves,
            vec![(
                "wrapper/Game.exe".to_string(),
                "Out/bin/wrapper/Game.exe".to_string()
            )],
            "the glob spells out no folder, so nothing is stripped"
        );
        assert!(
            output
                .reasoning
                .iter()
                .any(|line| line.contains("readme.txt") && line.contains("not carried")),
            "the file no pattern claimed is named rather than dropped in silence: {:?}",
            output.reasoning
        );
        assert_eq!(
            output.downloads[0].dest_path,
            "Out/Screenshots/image_001.jpg"
        );
    }

    /// The retired expander special-cased ` v$version`, dropping it when
    /// nothing knew the version so `"$title v$version"` degraded to
    /// `Title`. Expansion is template-driven now and an unresolved token
    /// in a name costs its output, so the same stored rule on the same
    /// archive produces no folder at all.
    ///
    /// What makes that a loss a user can act on rather than a silent one
    /// is the reason, so the reason is what this asserts: the skipped
    /// output must name `$version`. A test that only counted the outputs
    /// would pass just as well if the engine skipped it saying nothing.
    #[test]
    fn a_version_a_stored_rule_asked_for_and_nothing_set_skips_the_output_by_name() {
        let stored: OrganizationRule = serde_json::from_str(
            r#"{"id":0,"name":"Versioned","priority":1,"is_enabled":true,
                "trigger":{"metadata_source":null,"filename_pattern":null,"has_file":null},
                "actions":{"root_folder":"$title v$version","output_name":null,
                    "move_files":[],"use_standard_layout":true}}"#,
        )
        .expect("an old versioned rule must still deserialize");

        let meta = GameMetadata {
            product_id: "RJ999001".to_string(),
            source: "dlsite".to_string(),
            title: "Placeholder Game".to_string(),
            description: None,
            tags: vec![],
            release_date: None,
            creator: None,
            screenshots: vec![],
            metadata_json: "{}".to_string(),
        };

        // Nothing here carries a version: not the archive name, not the
        // folder the content root sits in.
        let entries = vec![make_entry("Placeholder Game/Game.exe", 10, false)];
        let plan = RuleEngine::create_plan(
            &stored,
            "Placeholder Game.zip",
            &entries,
            Some(&meta),
            &no_reads,
        )
        .expect("an unnameable output is a skip, not a failed plan");

        assert!(
            plan.outputs.is_empty(),
            "the name could not be resolved, so there is no folder to put anything in: {:?}",
            plan.outputs
                .iter()
                .map(|o| &o.root_folder)
                .collect::<Vec<_>>()
        );
        assert_eq!(plan.skipped_outputs.len(), 1);
        assert!(
            plan.skipped_outputs[0].1.contains("$version"),
            "the reason must name the token that went unset: {:?}",
            plan.skipped_outputs[0]
        );

        // And the same rule with a version in the archive name resolves,
        // so the skip is about the missing value and not about the
        // template being rejected outright.
        let named = RuleEngine::create_plan(
            &stored,
            "Placeholder Game v1.2.zip",
            &entries,
            Some(&meta),
            &no_reads,
        )
        .expect("plan");
        assert_eq!(named.outputs.len(), 1);
        assert_eq!(named.outputs[0].root_folder, "Placeholder Game v1.2");
    }

    /// A rule saved after the change carries its layout and is read
    /// back as written, with nothing translated on the way in.
    #[test]
    fn a_rule_stored_with_a_layout_is_read_as_written() {
        let actions: RuleActions = serde_json::from_str(
            r#"{"output_name":null,"layout":{"outputs":{"PerDirectoryContaining":{"marker":"modinfo.ini"}},
                "file_variables":[{"as_name":"mod_name","file":"modinfo.ini","key":"name"}],
                "name":"$mod_name","place":[{"from":"All","into":""}],
                "generate":[],"fetch":[]}}"#,
        )
        .expect("a current rule must deserialize");

        assert_eq!(
            actions.layout.outputs,
            OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string()
            }
        );
        assert_eq!(actions.layout.name, "$mod_name");
        assert_eq!(actions.layout.place[0].from, Source::All);
    }

    /// The round trip a saved rule takes through the database: serialize
    /// the current shape, read it back, and get the same layout. Without
    /// this the translation could quietly become the only path that
    /// works.
    #[test]
    fn a_layout_survives_being_serialized_and_read_back() {
        let actions = RuleActions {
            output_name: Some("$title.zip".to_string()),
            layout: standard_layout("[$product_id] $title"),
        };

        let json = serde_json::to_string(&actions).expect("serialize");
        let back: RuleActions = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.output_name.as_deref(), Some("$title.zip"));
        assert_eq!(back.layout, actions.layout);
    }
}
