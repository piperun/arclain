//! `RuleEngine` impl — rule matching, plan generation, glob matching.
//!
//! Was the bulk of the old single-file `engine.rs`. Helpers like the
//! tree pruner live in [`super::tree`]; this file focuses on plan
//! construction. `matches_glob` is `pub(super)` so the test suite in
//! `mod.rs` can exercise it directly without duplicating fixtures.
//!
//! [`fill_output`] is the second half of resolving a layout: given an
//! output that [`super::outputs`] has already located and named, it
//! works out what goes into it. The two halves are separate because
//! naming reads a handful of files out of the archive and filling reads
//! none — a folder name must never wait on a payload.

use super::outputs::{expand, resolve_outputs, ResolvedOutput};
use super::tree::TreeNode;
use super::{OrganizationPlan, PendingDownload, PlannedOutput, RuleEngine, StagedContent};
use crate::features::organization::layout::{
    FetchSource, GeneratedContent, Layout, OutputSelector, Placement, Source,
};
use crate::features::organization::metadata::{GameMetadata, ScreenshotData};
use crate::features::organization::{OrganizationRule, RuleTrigger};
use crate::ArchiveEntry;
use anyhow::{bail, Context, Result};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

    /// Generate an organization plan based on a rule.
    ///
    /// The two halves of resolving a layout meet here: [`resolve_outputs`]
    /// works out how many outputs there are and what each is called, and
    /// [`fill_output`] works out what goes into each one.
    ///
    /// `read_entry` is handed the archive-relative path of a file a
    /// layout named — a `modinfo.ini` behind a file variable — and
    /// returns its bytes. Nothing else is read: planning must not pull a
    /// payload out of an archive to decide a folder name. Taking a
    /// closure rather than an `Archive` is what lets a directory be a new
    /// input rather than a new engine.
    pub fn create_plan(
        rule: &OrganizationRule,
        input_name: &str,
        entries: &[ArchiveEntry],
        game_metadata: Option<&crate::features::organization::metadata::GameMetadata>,
        read_entry: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<OrganizationPlan> {
        // Prune unnecessary files/folders before any analysis.
        let pruned_entries = Self::prune_entries(entries);
        let entries = &pruned_entries;
        let layout = &rule.actions.layout;

        // A content root's folder name is one of the variable sources —
        // `$version`, `$root_tags`, `$folder_name` are read out of it —
        // and an output's name is resolved before anything is placed, so
        // the detection has to happen here as well as inside each
        // output.
        //
        // Only when the whole input is one output, though. Detection
        // across the input is scoped to nothing in particular, and
        // feeding that into a per-folder output's *name* would let one
        // mod's folder name be decided by what its siblings contain.
        // Placement does not have this problem: `fill_output` detects
        // within each output. A marker layout wanting `$version` in a
        // name therefore finds it unset and says so, which is the loud
        // failure rather than the quiet wrong answer.
        //
        // The two conditions together are exactly what the retired
        // `use_standard_layout` boolean expressed, since it only ever
        // existed alongside one output.
        let names_a_content_root = layout
            .place
            .iter()
            .any(|placement| matches!(placement.from, Source::ContentRoot));
        let content_root = (names_a_content_root
            && matches!(layout.outputs, OutputSelector::Whole))
        .then(|| Self::find_game_content_root_in_entries(entries));

        let base_variables =
            Self::build_metadata_map(rule, input_name, game_metadata, &content_root);

        let resolved = resolve_outputs(layout, entries, &base_variables, read_entry)?;

        let mut outputs = Vec::with_capacity(resolved.outputs.len());
        for output in &resolved.outputs {
            outputs.push(fill_output(layout, output, entries, game_metadata)?);
        }

        let mut plan = OrganizationPlan {
            rule_name: rule.name.clone(),
            outputs,
            skipped_outputs: resolved.skipped,
        };
        // Validated over every output the layout resolved to, before any
        // of them is set aside below. An output that carries no file
        // still names a root folder, and a root reaching out of the work
        // directory is a fault in the layout whether or not anything
        // would have been written through it.
        plan.validate_paths()
            .context("organization plan path validation")?;
        report_outputs_that_carry_nothing(&mut plan, &resolved.outputs);
        Ok(plan)
    }

    /// Build the variable map every one of a layout's templates starts
    /// from — the output name and each `into`. Pulls from (in order,
    /// last write wins): GameMetadata fields, flattened
    /// `metadata_json`, named captures from `trigger.filename_pattern`,
    /// the archive filename version regex, and the inner content-root
    /// folder name (version + bracketed tags).
    ///
    /// A layout's own file variables are resolved per output on top of
    /// this map, because two outputs of one input read two different
    /// files and so cannot share them.
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

    /// Whether `path` matches `pattern`. Understands `**`, `*.ext`,
    /// `dir/**`, an exact path, and a bare filename anywhere in the tree.
    ///
    /// The `dir/**` branch is newer than the rest, and widens what
    /// already-saved rules match. Before it, every `dir/**` pattern
    /// returned false for every path, so a stored rule written that way
    /// matched nothing and its files fell through to the `game/` default
    /// target. They now route where the rule says. No file is dropped
    /// either way, but a user with such a rule will see their files move
    /// somewhere new.
    ///
    /// That branch folds ASCII case, deliberately, because the `*.ext`
    /// branch beside it already does and one function answering two ways
    /// is worse than either answer: `docs/**` matches `Docs/x`. The
    /// exact and filename branches stay case-sensitive, which is the
    /// behaviour they shipped with and not something to change here.
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

        // `dir/**` — everything under a named folder. Without this a
        // glob naming a folder falls through to the exact and filename
        // checks, neither of which an entry inside that folder can
        // satisfy, so a placement written this way would place nothing
        // and say nothing about why.
        if let Some(directory) = pattern.strip_suffix("/**") {
            return under_directory(&path.replace('\\', "/"), directory);
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
        Self::detect_content_root(entries).path
    }

    /// The same detection, keeping what convinced it. One scorer, so a
    /// preview can never describe a folder the plan did not pick.
    fn detect_content_root(entries: &[ArchiveEntry]) -> DetectedContentRoot {
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

        // Group entries by parent directory - track both standard indicators and any .exe
        let mut named: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
        // One executable per directory, the lexicographically smallest,
        // so the evidence names a fixed file rather than whichever the
        // iteration order reached first.
        let mut executables: BTreeMap<PathBuf, String> = BTreeMap::new();

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
                        named
                            .entry(parent.to_path_buf())
                            .or_default()
                            .push(fname_str.to_string());
                    }

                    // Check for any .exe file (flexible indicator)
                    if !entry.is_dir && fname_str.to_lowercase().ends_with(".exe") {
                        executables
                            .entry(parent.to_path_buf())
                            .and_modify(|smallest| {
                                if fname_str.as_ref() < smallest.as_str() {
                                    *smallest = fname_str.to_string();
                                }
                            })
                            .or_insert_with(|| fname_str.to_string());
                    }
                }
            }
        }

        // `www`, `data` and `js` are folder names, and a folder never
        // reaches this function as an entry: `create_plan` scores the
        // pruned list, and `TreeNode::flatten` keeps files only. Derive
        // the folders the file paths imply and score them by name, once
        // each -- crediting per file would let a folder holding five
        // hundred files outscore the layout it sits in.
        let mut implied_dirs: BTreeSet<PathBuf> = BTreeSet::new();
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

        // Each indicator is worth one, so the evidence list is the score
        // written out. Assembled in three passes rather than in entry
        // order, and sorted within each, so the same archive reads the
        // same way every run.
        let mut scored: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
        for (dir, mut names) in named {
            names.sort();
            scored.entry(dir).or_default().extend(names);
        }
        for (dir, executable) in executables {
            scored
                .entry(dir)
                .or_default()
                .push(format!("an executable ({executable})"));
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
                scored
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(format!("a {name}/ folder"));
            }
        }

        // Sort first: `>` alone would let two directories tied at the
        // best score resolve differently each time. Ties go to the
        // lexicographically smallest path.
        let mut ranked: Vec<(PathBuf, Vec<String>)> = scored.into_iter().collect();
        ranked.sort_by(|(left_path, left), (right_path, right)| {
            right
                .len()
                .cmp(&left.len())
                .then_with(|| left_path.cmp(right_path))
        });
        if let Some((dir, evidence)) = ranked.into_iter().next() {
            if evidence.len() >= 2 {
                return DetectedContentRoot {
                    path: dir,
                    score: evidence.len(),
                    evidence,
                };
            }
        }

        // If no definitive root found (score < 2), fallback to common root or just root
        // Find common root logic could be reused here or simple fallback
        // For now, if we can't find game content, we assume content is at root
        // of the *entries* (common prefix)
        let paths: Vec<&Path> = entries.iter().map(|e| Path::new(&e.path)).collect();
        let mut common = PathBuf::new();
        if !paths.is_empty() {
            let mut iter = paths.iter();
            common = iter
                .next()
                .unwrap()
                .parent()
                .unwrap_or(Path::new(""))
                .to_path_buf();
            for path in iter {
                while !path.starts_with(&common) {
                    if !common.pop() {
                        break;
                    }
                }
            }
        }

        DetectedContentRoot {
            path: common,
            score: 0,
            evidence: Vec::new(),
        }
    }
}

/// The folder that looks like the payload, and what made it look that
/// way. The evidence is what a preview shows, so a detection a user
/// disagrees with is one they can check rather than only distrust.
struct DetectedContentRoot {
    path: PathBuf,
    /// How many indicators the folder scored. Below two nothing was
    /// convincing enough to call a payload: the path is then the common
    /// prefix of the entries and the evidence is empty.
    score: usize,
    evidence: Vec<String>,
}

/// What goes into one output: every file placed in it, every file
/// written into it, every image fetched into it, and the reasoning
/// behind all three. The other half of resolving a layout, which named
/// the output and located it, is [`super::outputs`].
///
/// `entries` is the whole input. Scoping it to `output.root` happens
/// here, so two outputs of one archive answer every question — which
/// glob matches, where the content root is — about their own contents
/// and never about each other's.
///
/// Placements are evaluated in order and the first that matches a file
/// claims it; no later placement sees that file again. A file no
/// placement matched is not carried into the output at all, and both
/// what each placement did and what was left behind land in `reasoning`
/// so neither is silent.
fn fill_output(
    layout: &Layout,
    output: &ResolvedOutput,
    entries: &[ArchiveEntry],
    metadata: Option<&GameMetadata>,
) -> Result<PlannedOutput> {
    refuse_unreachable_placements(&layout.place)?;

    let mut reasoning = Vec::new();
    let scoped = scope_entries(&output.root, entries);
    let mut claimed = vec![false; scoped.len()];
    let mut moves = Vec::new();
    // Every destination this output writes to, so two files cannot land
    // on one path. Nothing validates a `PlannedOutput` on its way out
    // yet, so this is the only thing standing between a layout that
    // cannot mean what it says and one file quietly overwriting another.
    let mut destinations: BTreeMap<String, String> = BTreeMap::new();

    for placement in &layout.place {
        // The part of a matched path the placement located rather than
        // carried: `into` stands in for it and the rest is appended.
        let located = match &placement.from {
            Source::All => String::new(),
            Source::Matching(glob) => literal_directory_prefix(glob),
            Source::ContentRoot => {
                // Scored over this output's entries only. Run across the
                // whole input, two mod folders would compete and one of
                // them would end up carrying nothing.
                let within: Vec<ArchiveEntry> =
                    scoped.iter().map(|file| file.relative.clone()).collect();
                let detected = RuleEngine::detect_content_root(&within);
                reasoning.push(describe_content_root(&detected));
                detected.path.to_string_lossy().replace('\\', "/")
            }
        };

        let (into, unset) = expand(&placement.into, &output.variables);
        note_unset(&mut reasoning, "destination", &placement.into, &unset);

        let mut placed = 0usize;
        for (index, file) in scoped.iter().enumerate() {
            if claimed[index] {
                continue;
            }
            let path = &file.relative.path;
            let matched = match &placement.from {
                Source::All => true,
                Source::Matching(glob) => RuleEngine::matches_glob(glob, path),
                Source::ContentRoot => located.is_empty() || under_directory(path, &located),
            };
            if !matched {
                continue;
            }

            claimed[index] = true;
            placed += 1;
            let destination =
                join_destination(&[&output.name, &into, strip_directory(path, &located)]);
            claim_destination(&mut destinations, &destination)?;
            moves.push((file.source.clone(), destination));
        }

        // Said even when there was nothing to infer. A preview with no
        // explanation at all is the explainability property failing, and
        // it would fail on the commonest layout of the lot: one `All`
        // placement, a name that resolves, nothing left over.
        reasoning.push(format!(
            "{} placed {} into {}",
            describe_source(&placement.from),
            count_files(placed),
            describe_destination(&output.name, &into)
        ));
    }

    let left: Vec<&str> = scoped
        .iter()
        .zip(&claimed)
        .filter(|(_, carried)| !**carried)
        .map(|(file, _)| file.relative.path.as_str())
        .collect();
    if !left.is_empty() {
        // Named, not just counted: "47 files were not carried" is a
        // number a user cannot check, and the first few paths are enough
        // to tell whether the right ones were dropped.
        let shown = left.len().min(3);
        let rest = left.len() - shown;
        reasoning.push(format!(
            "{} matched no placement and {} not carried: {}{}",
            count_files(left.len()),
            if left.len() == 1 { "was" } else { "were" },
            left[..shown].join(", "),
            if rest > 0 {
                format!(", and {rest} more")
            } else {
                String::new()
            }
        ));
    }

    let mut generated_files = Vec::new();
    for generated in &layout.generate {
        match generated.content {
            GeneratedContent::MetadataDocument => {
                let Some(document) = metadata.and_then(metadata_document) else {
                    continue;
                };
                let (into, unset) = expand(&generated.into, &output.variables);
                note_unset(&mut reasoning, "generated file", &generated.into, &unset);
                let destination = join_destination(&[&output.name, &into]);
                claim_destination(&mut destinations, &destination)?;
                generated_files.push((destination, document));
            }
        }
    }

    let mut downloads = Vec::new();
    for fetched in &layout.fetch {
        match fetched.source {
            FetchSource::Screenshots => {
                let Some(game) = metadata else {
                    continue;
                };
                let is_dlsite = game.source.eq_ignore_ascii_case("dlsite");
                let (into, unset) = expand(&fetched.into, &output.variables);
                note_unset(&mut reasoning, "fetch destination", &fetched.into, &unset);

                for (index, screenshot) in game.screenshots.iter().enumerate() {
                    // Only URL screenshots are downloadable; a plugin
                    // that already fetched the file, or inlined the
                    // bytes, reports a form this plan cannot schedule.
                    // Its position is spent all the same, so names and
                    // cache keys stay lined up with the list the
                    // provider handed over.
                    let ScreenshotData::Url(url) = screenshot else {
                        continue;
                    };

                    let mut variables = output.variables.clone();
                    // Padded to three, so ten screenshots still sort in
                    // order. The extension comes from the URL rather than
                    // from the template, because a template that spells
                    // one out renames every `.png` the source serves.
                    variables.insert("index".to_string(), format!("{:03}", index + 1));
                    variables.insert("ext".to_string(), url_extension(url));
                    let (name, unset) = expand(&fetched.name, &variables);
                    note_unset(&mut reasoning, "fetched file name", &fetched.name, &unset);

                    // Cache key must match gameta's cache_keys format.
                    let cache_key = if is_dlsite {
                        format!("dlsite:{}:screenshot_{}", game.product_id, index)
                    } else {
                        format!("screenshot:{}:{}", game.product_id, index)
                    };

                    let destination = join_destination(&[&output.name, &into, &name]);
                    claim_destination(&mut destinations, &destination)?;
                    downloads.push(PendingDownload {
                        product_id: is_dlsite.then(|| game.product_id.clone()),
                        url: url.clone(),
                        dest_path: destination,
                        cache_key,
                        cached: false, // Will be checked by UI when loading
                    });
                }
            }
        }
    }

    // Reasoning is read, not counted. Two placements that fail to
    // resolve the same token have one thing to say between them.
    let mut said = BTreeSet::new();
    reasoning.retain(|line| said.insert(line.clone()));

    Ok(PlannedOutput {
        root_folder: output.name.clone(),
        root_folder_template: layout.name.clone(),
        moves,
        generated_files,
        downloads,
        resolved_variables: output.variables.clone(),
        reasoning,
    })
}

/// One file under an output's root.
struct ScopedEntry {
    /// Its path in the archive, which is what a move reads from.
    source: String,
    /// The same file with the output's root taken off, which is what
    /// placements match and what destinations are built from. Carried
    /// as an entry because content-root detection scores a list of them.
    relative: ArchiveEntry,
}

/// The files under `root`, each rewritten relative to it. Directories
/// are dropped: a list of files describes the same tree, and an empty
/// folder is not content anyone asked to carry.
///
/// This is where the input is partitioned between outputs, so the
/// comparison is [`under_root`]'s exact one and not `under_directory`'s
/// folded one.
fn scope_entries(root: &Path, entries: &[ArchiveEntry]) -> Vec<ScopedEntry> {
    let root = root.to_string_lossy().replace('\\', "/");
    let mut scoped = Vec::new();

    for entry in entries {
        if entry.is_dir {
            continue;
        }
        let path = entry.path.replace('\\', "/");
        let relative = if root.is_empty() {
            path
        } else if under_root(&path, &root) {
            path[root.len() + 1..].to_string()
        } else {
            continue;
        };

        scoped.push(ScopedEntry {
            source: entry.path.clone(),
            relative: ArchiveEntry {
                path: relative,
                ..entry.clone()
            },
        });
    }

    scoped
}

/// A placement after one that takes everything can never match. Refusing
/// the layout says so once, where leaving it in would quietly drop
/// whatever the author meant to route through it.
fn refuse_unreachable_placements(place: &[Placement]) -> Result<()> {
    for (index, placement) in place.iter().enumerate() {
        let after = place.len() - index - 1;
        if matches!(placement.from, Source::All) && after > 0 {
            bail!(
                "placement {} takes everything under the output's root, which leaves the {} \
                 after it unreachable",
                index + 1,
                if after == 1 {
                    "placement".to_string()
                } else {
                    format!("{after} placements")
                }
            );
        }
    }
    Ok(())
}

/// Take a destination for this output, refusing a second file that
/// wants the same one. Two placements routing into one folder can send
/// two same-named files to one path, and a plan that does that quietly
/// loses whichever file the applier writes first.
///
/// Compared through `to_uppercase`, the way `OrganizationPlan::validate_paths`
/// keys destinations. That is nobody's filesystem table, so it refuses a
/// little more than any one filesystem would merge — the right side to
/// err on, since refusing a separable pair costs a message and merging
/// two files costs one of them.
fn claim_destination(taken: &mut BTreeMap<String, String>, destination: &str) -> Result<()> {
    let Some(first) = taken.insert(destination.to_uppercase(), destination.to_string()) else {
        return Ok(());
    };
    if first == destination {
        bail!("two files land on the same destination: {destination:?}");
    }
    bail!("two files land on destinations a filesystem may merge: {first:?} and {destination:?}");
}

/// Where a placement drew its files from, in a line a preview can show.
fn describe_source(from: &Source) -> String {
    match from {
        Source::All => "everything under the output's root".to_string(),
        Source::Matching(glob) => format!("the glob {glob:?}"),
        Source::ContentRoot => "the content root".to_string(),
    }
}

/// Where a placement put them, naming the output's own root rather than
/// showing an empty string for it.
fn describe_destination(name: &str, into: &str) -> String {
    let destination = join_destination(&[name, into]);
    if destination.is_empty() {
        "the output's root".to_string()
    } else {
        destination
    }
}

/// Move every output that would put nothing on disk onto the plan's
/// `skipped_outputs`, with the folder it would have been.
///
/// An output's folder is written by writing the files that go into it,
/// so one carrying no file, no generated document and no fetched image
/// produces no folder at all. Left among the outputs it is a promise of
/// a folder nobody will find, with nothing anywhere saying why — and a
/// plan whose every output is like that empties the work directory when
/// applied. Its siblings are unaffected, which is why this reports
/// rather than fails: a mod pack with one bare folder still organizes
/// the rest, and says which one it passed over.
///
/// `located` is the outputs resolution produced, in the same order, so
/// each report can name the folder *inside the input* the user should
/// go and look at rather than only the one that will not appear.
fn report_outputs_that_carry_nothing(plan: &mut OrganizationPlan, located: &[ResolvedOutput]) {
    let filled = std::mem::take(&mut plan.outputs);
    // One filled output per located one, in order. The zip below would
    // otherwise drop the tail of the longer list without a word.
    debug_assert_eq!(filled.len(), located.len());
    let mut kept = Vec::with_capacity(filled.len());

    for (output, location) in filled.into_iter().zip(located) {
        if !output.stages_nothing() {
            kept.push(output);
            continue;
        }
        plan.skipped_outputs.push((
            location.root.to_string_lossy().replace('\\', "/"),
            format!(
                "nothing was placed into {}, and the layout generates and fetches nothing into \
                 it, so the run would produce no such folder",
                describe_output_folder(&output.root_folder)
            ),
        ));
    }

    plan.outputs = kept;
}

/// An output's folder as a reason should name it. An empty name is a
/// real answer — the output has no wrapper and its content sits at the
/// top level — and quoting it prints nothing at all.
fn describe_output_folder(name: &str) -> String {
    if name.is_empty() {
        "the top level of the result".to_string()
    } else {
        format!("{name:?}")
    }
}

/// A file count that reads as a sentence rather than as a number.
fn count_files(count: usize) -> String {
    match count {
        0 => "nothing".to_string(),
        1 => "1 file".to_string(),
        many => format!("{many} files"),
    }
}

/// The extension a download's source URL carries, or `jpg` when it names
/// none. Derived from the URL and not from the template, so a source
/// that serves a `.png` is not saved under a `.jpg` name.
fn url_extension(url: &str) -> String {
    Path::new(url)
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned())
        .unwrap_or_else(|| "jpg".to_string())
}

/// The folders a glob spells out. `assets/**` names `assets` the way a
/// content root names a folder — `into` stands in for it and what the
/// wildcards matched is appended — so two files of the same name in
/// different folders stay apart. A glob with no folder in it, `*.pdf`
/// or `readme.txt`, names none and nothing is stripped.
fn literal_directory_prefix(glob: &str) -> String {
    let Some((directories, _)) = glob.rsplit_once('/') else {
        return String::new();
    };

    let mut spelled = Vec::new();
    for segment in directories.split('/') {
        if segment.contains('*') || segment.contains('?') {
            break;
        }
        spelled.push(segment);
    }
    spelled.join("/")
}

/// Whether `path` sits inside the directory `prefix`. ASCII case folds,
/// as it already does in `matches_glob`'s extension branch, so one glob
/// does not answer two ways depending on how an archive happened to
/// spell a folder. Folding only ASCII keeps the comparison
/// length-preserving, which is what lets a caller strip exactly what
/// matched. A folder is not a file inside itself, so `prefix` alone
/// does not sit under `prefix`.
///
/// For a glob's prefix, which a user wrote and may have spelled unlike
/// the archive. Not for an output's root — see [`under_root`], which
/// answers the same question about a prefix the archive itself spelled.
fn under_directory(path: &str, prefix: &str) -> bool {
    let head = prefix.len();
    path.len() > head
        && path.is_char_boundary(head)
        && path[..head].eq_ignore_ascii_case(prefix)
        && path.as_bytes()[head] == b'/'
}

/// Whether `path` sits inside the output root `root`, compared byte for
/// byte.
///
/// A root is not a pattern. `select_roots` cut it out of an entry path
/// in this same archive, so byte-for-byte is the only comparison that
/// answers what the archive actually says. Folding case here decides
/// that `MOD/body.pak` sits under the root `Mod` as well as under its
/// own, and the placements of both outputs then claim it: one file
/// copied into two folders, onto destinations that genuinely differ so
/// `OrganizationPlan::validate_paths` sees nothing wrong and nothing
/// says it was carried twice. `refuse_nested_roots` compares roots
/// exactly for the same reason, and the two have to give one answer or
/// the invariant it enforces is not the one scoping applies.
///
/// The distinction is drawn here rather than at every prefix comparison
/// because this is the only one that partitions files *between*
/// outputs. A folded answer anywhere downstream stays inside one output
/// and lands on a destination collision the plan refuses out loud.
fn under_root(path: &str, root: &str) -> bool {
    let head = root.len();
    path.len() > head && path.starts_with(root) && path.as_bytes()[head] == b'/'
}

/// Drop `prefix` and its separator from the front of `path`. A path the
/// prefix does not cover comes back whole rather than mangled.
fn strip_directory<'a>(path: &'a str, prefix: &str) -> &'a str {
    if prefix.is_empty() || !under_directory(path, prefix) {
        return path;
    }
    &path[prefix.len() + 1..]
}

/// Join the pieces of a destination, dropping every one that names
/// nothing. An empty output name means no wrapper folder and an empty
/// `into` means the output's own root, so either may collapse rather
/// than leave a doubled or leading separator; `.` is what the older
/// vocabulary wrote for the same thing, and a `.` component is refused
/// outright where the plan meets the filesystem.
fn join_destination(parts: &[&str]) -> String {
    let mut joined = String::new();

    for part in parts {
        for segment in part.split(['/', '\\']) {
            if segment.is_empty() || segment == "." {
                continue;
            }
            if !joined.is_empty() {
                joined.push('/');
            }
            joined.push_str(segment);
        }
    }

    joined
}

/// The document a `MetadataDocument` writes: the layered document the
/// provider produced, which carries the source-specific fields the
/// extracted struct does not keep -- `metadata_json` is
/// `#[serde(skip)]`, so serializing the struct silently drops them.
/// Fall back to the struct only when no raw document came with it.
fn metadata_document(metadata: &GameMetadata) -> Option<String> {
    if metadata.metadata_json.trim().is_empty() {
        serde_json::to_string_pretty(metadata).ok()
    } else {
        Some(metadata.metadata_json.clone())
    }
}

/// Record a template that could not be filled in. A name that will not
/// resolve costs its output, because a folder nobody can trace back to
/// a mod is worse than a reported gap. A destination inside an output
/// is not that: the files are still traceable, so the token is left
/// standing where a preview shows it and the reason is written down.
fn note_unset(reasoning: &mut Vec<String>, what: &str, template: &str, unset: &[String]) {
    if unset.is_empty() {
        return;
    }
    let tokens: Vec<String> = unset.iter().map(|token| format!("${token}")).collect();
    reasoning.push(format!(
        "the {what} {template:?} needs {}, which nothing set",
        tokens.join(", ")
    ));
}

/// What a content-root detection found, in a line a preview can show.
fn describe_content_root(detected: &DetectedContentRoot) -> String {
    let shown = detected.path.display().to_string();
    let folder = if shown.is_empty() {
        "the output's root".to_string()
    } else {
        shown
    };

    if detected.evidence.is_empty() {
        return format!("nothing scored as a content root, so {folder} is the payload");
    }
    format!(
        "content root {folder} scored {} on {}",
        detected.score,
        detected.evidence.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::ArchiveEntry;
    use crate::features::organization::layout::{
        FetchSource, Fetched, Generated, GeneratedContent, Layout, Placement, Source,
    };
    use crate::features::organization::metadata::{GameMetadata, ScreenshotData};

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

    fn output(name: &str, root: &str) -> ResolvedOutput {
        ResolvedOutput {
            root: PathBuf::from(root),
            name: name.to_string(),
            variables: HashMap::new(),
        }
    }

    fn rule_with(layout: Layout) -> OrganizationRule {
        OrganizationRule {
            name: "Placeholder Rule".to_string(),
            priority: 10,
            is_enabled: true,
            trigger: RuleTrigger::default(),
            actions: crate::features::organization::RuleActions {
                output_name: None,
                layout,
            },
            ..Default::default()
        }
    }

    fn no_reads(_: &str) -> Option<Vec<u8>> {
        None
    }

    fn metadata(document: &str) -> GameMetadata {
        GameMetadata {
            product_id: "RJ123456".to_string(),
            source: "dlsite".to_string(),
            title: "Placeholder Game".to_string(),
            description: None,
            tags: vec![],
            release_date: None,
            creator: Some("Placeholder Circle".to_string()),
            screenshots: vec![],
            metadata_json: document.to_string(),
        }
    }

    /// Staging builds an output's folder by writing the files that go
    /// into it, so an output carrying none is not an empty folder — it
    /// is no folder at all, and until now nothing anywhere said so. It
    /// belongs beside the outputs that could not be named: reported, and
    /// not planned.
    #[test]
    fn an_output_that_carries_no_file_is_reported_rather_than_vanishing() {
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![crate::features::organization::layout::FileVariable {
                as_name: "mod_name".to_string(),
                file: "modinfo.ini".to_string(),
                key: "name".to_string(),
            }],
            name: "$mod_name".to_string(),
            place: vec![Placement {
                from: Source::Matching("natives/**".to_string()),
                into: String::new(),
            }],
            ..Layout::default()
        };
        let entries = vec![
            entry("Full/modinfo.ini"),
            entry("Full/natives/body.pak"),
            // Carries the marker, so it resolves and is named, but the
            // layout's one placement claims nothing inside it.
            entry("Bare/modinfo.ini"),
        ];
        let read = |path: &str| -> Option<Vec<u8>> {
            match path {
                "Full/modinfo.ini" => Some(b"name=Placeholder Mod".to_vec()),
                "Bare/modinfo.ini" => Some(b"name=Placeholder Addon".to_vec()),
                _ => None,
            }
        };

        let plan = RuleEngine::create_plan(&rule_with(layout), "pack.zip", &entries, None, &read)
            .expect("plan");

        let planned: Vec<_> = plan
            .outputs
            .iter()
            .map(|output| output.root_folder.clone())
            .collect();
        assert_eq!(
            planned,
            vec!["Placeholder Mod".to_string()],
            "an output that would produce no folder must not be planned"
        );

        let (root, reason) = plan
            .skipped_outputs
            .first()
            .expect("the empty output must be reported");
        assert_eq!(root, "Bare");
        assert!(
            reason.contains("Placeholder Addon"),
            "the reason must name the folder that went missing: {reason}"
        );
        assert_eq!(plan.skipped_outputs.len(), 1);
    }

    /// Which output a file belongs to is settled by comparing its path
    /// against each output's root. Folding case there answers that a
    /// file under `MOD` also sits under `Mod`, so both outputs'
    /// placements claim it and the applier copies it into both folders.
    /// The destinations genuinely differ, so `validate_paths` sees
    /// nothing wrong and nothing says the file was carried twice.
    #[test]
    fn two_roots_spelled_in_different_cases_each_carry_only_their_own() {
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![crate::features::organization::layout::FileVariable {
                as_name: "mod_name".to_string(),
                file: "modinfo.ini".to_string(),
                key: "name".to_string(),
            }],
            name: "$mod_name".to_string(),
            // Not `All`: the marker is called `modinfo.ini` under either
            // root, and two of those landing on one destination is a
            // collision the output already refuses out loud. The silent
            // fault is the one worth pinning.
            place: vec![Placement {
                from: Source::Matching("*.pak".to_string()),
                into: String::new(),
            }],
            ..Layout::default()
        };
        let entries = vec![
            entry("Mod/modinfo.ini"),
            entry("Mod/lower.pak"),
            entry("MOD/modinfo.ini"),
            entry("MOD/upper.pak"),
        ];
        // Distinct names, so the two outputs cannot collide on a name and
        // scoping is the only thing under test.
        let read = |path: &str| -> Option<Vec<u8>> {
            match path {
                "Mod/modinfo.ini" => Some(b"name=Placeholder Mod".to_vec()),
                "MOD/modinfo.ini" => Some(b"name=Placeholder Addon".to_vec()),
                _ => None,
            }
        };

        let plan = RuleEngine::create_plan(&rule_with(layout), "pack.zip", &entries, None, &read)
            .expect("two sibling folders are two outputs");

        let mut carried: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for output in &plan.outputs {
            for (source, _) in &output.moves {
                carried
                    .entry(source.as_str())
                    .or_default()
                    .push(output.root_folder.as_str());
            }
        }
        let twice: Vec<_> = carried.iter().filter(|(_, into)| into.len() > 1).collect();
        assert!(
            twice.is_empty(),
            "no file may be carried into two outputs: {twice:?}"
        );
        assert_eq!(
            carried.get("Mod/lower.pak").map(Vec::as_slice),
            Some(["Placeholder Mod"].as_slice())
        );
        assert_eq!(
            carried.get("MOD/upper.pak").map(Vec::as_slice),
            Some(["Placeholder Addon"].as_slice())
        );
    }

    /// The nesting refusal compares roots exactly, so `Mod` and
    /// `MOD/Inner` are two unrelated subtrees and it lets them through.
    /// Scoping has to answer the same way: folding case there makes
    /// `Mod`'s output claim everything under `MOD/Inner` as well, which
    /// is the duplication the refusal exists to prevent, arriving
    /// through the door the refusal was told to leave open.
    #[test]
    fn a_root_nested_under_another_spelling_is_not_claimed_by_it() {
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![crate::features::organization::layout::FileVariable {
                as_name: "mod_name".to_string(),
                file: "modinfo.ini".to_string(),
                key: "name".to_string(),
            }],
            name: "$mod_name".to_string(),
            place: vec![Placement {
                from: Source::Matching("*.pak".to_string()),
                into: String::new(),
            }],
            ..Layout::default()
        };
        let entries = vec![
            entry("Mod/modinfo.ini"),
            entry("Mod/outer.pak"),
            entry("MOD/Inner/modinfo.ini"),
            entry("MOD/Inner/inner.pak"),
        ];
        let read = |path: &str| -> Option<Vec<u8>> {
            match path {
                "Mod/modinfo.ini" => Some(b"name=Placeholder Mod".to_vec()),
                "MOD/Inner/modinfo.ini" => Some(b"name=Placeholder Addon".to_vec()),
                _ => None,
            }
        };

        let plan = RuleEngine::create_plan(&rule_with(layout), "pack.zip", &entries, None, &read)
            .expect("neither root sits inside the other");

        let sources: Vec<Vec<&str>> = plan
            .outputs
            .iter()
            .map(|output| {
                output
                    .moves
                    .iter()
                    .map(|(source, _)| source.as_str())
                    .collect()
            })
            .collect();
        assert_eq!(
            sources,
            vec![vec!["MOD/Inner/inner.pak"], vec!["Mod/outer.pak"]]
        );
    }

    /// A rule the shipped editor produces today: a new rule starts from
    /// a default layout, which has no placements at all. It resolves to
    /// one named output carrying nothing, and applying that plan used to
    /// empty the work directory.
    #[test]
    fn a_layout_that_places_nothing_plans_no_output_at_all() {
        let layout = Layout {
            name: "Organized".to_string(),
            ..Layout::default()
        };
        let entries = vec![entry("game.exe")];

        let plan =
            RuleEngine::create_plan(&rule_with(layout), "game.zip", &entries, None, &no_reads)
                .expect("plan");

        assert!(
            plan.outputs.is_empty(),
            "a layout placing nothing produces nothing: {:?}",
            plan.outputs
        );
        assert!(plan.stages_nothing());
        let (root, reason) = plan
            .skipped_outputs
            .first()
            .expect("the run must say why it produces nothing");
        assert_eq!(root, "", "the whole input was the candidate");
        assert!(
            reason.contains("Organized"),
            "the reason must name the folder nobody will find: {reason}"
        );
    }

    #[test]
    fn the_first_placement_that_matches_claims_a_file() {
        // Two placements both match `docs/manual.pdf`; the first wins and
        // the file is not also copied by the second.
        let layout = Layout {
            name: "Out".to_string(),
            place: vec![
                Placement {
                    from: Source::Matching("docs/**".to_string()),
                    into: "Docs".to_string(),
                },
                Placement {
                    from: Source::All,
                    into: "Everything".to_string(),
                },
            ],
            ..Layout::default()
        };
        let entries = vec![entry("docs/manual.pdf"), entry("bin/app.exe")];
        let output = ResolvedOutput {
            root: PathBuf::new(),
            name: "Out".to_string(),
            variables: HashMap::new(),
        };

        let filled = fill_output(&layout, &output, &entries, None).expect("fill");
        let destinations: Vec<_> = filled.moves.iter().map(|(_, to)| to.clone()).collect();

        assert!(destinations.contains(&"Out/Docs/manual.pdf".to_string()));
        assert!(destinations.contains(&"Out/Everything/bin/app.exe".to_string()));
        assert_eq!(
            destinations
                .iter()
                .filter(|d| d.ends_with("manual.pdf"))
                .count(),
            1,
            "a file claimed by the first placement must not be placed twice: {destinations:?}"
        );
    }

    #[test]
    fn a_placement_after_an_all_placement_is_refused() {
        let layout = Layout {
            name: "Out".to_string(),
            place: vec![
                Placement {
                    from: Source::All,
                    into: String::new(),
                },
                Placement {
                    from: Source::Matching("*.pdf".to_string()),
                    into: "Docs".to_string(),
                },
            ],
            ..Layout::default()
        };
        let output = ResolvedOutput {
            root: PathBuf::new(),
            name: "Out".to_string(),
            variables: HashMap::new(),
        };

        let error = fill_output(&layout, &output, &[entry("a.pdf")], None)
            .expect_err("a placement after All can never match");
        assert!(format!("{error:#}").contains("unreachable"), "{error:#}");
    }

    #[test]
    fn a_file_matched_by_no_placement_is_not_carried() {
        let layout = Layout {
            name: "Out".to_string(),
            place: vec![Placement {
                from: Source::Matching("keep/**".to_string()),
                into: String::new(),
            }],
            ..Layout::default()
        };
        let entries = vec![entry("keep/wanted.bin"), entry("drop/unwanted.bin")];
        let output = ResolvedOutput {
            root: PathBuf::new(),
            name: "Out".to_string(),
            variables: HashMap::new(),
        };

        let filled = fill_output(&layout, &output, &entries, None).expect("fill");
        assert_eq!(filled.moves.len(), 1);
        assert!(filled.moves[0].1.ends_with("wanted.bin"));
    }

    /// A glob that spells a folder out locates a subtree the way a
    /// content root does, so `into` renames that folder rather than
    /// nesting under it — and everything below it keeps its shape. The
    /// alternative, flattening to the file name, would collide two
    /// same-named files from different folders into one destination.
    #[test]
    fn a_glob_that_names_a_folder_replaces_it_and_keeps_the_tree_under_it() {
        let layout = Layout {
            name: "Out".to_string(),
            place: vec![Placement {
                from: Source::Matching("assets/**".to_string()),
                into: "Media".to_string(),
            }],
            ..Layout::default()
        };
        let entries = vec![entry("assets/art/logo.png"), entry("assets/sfx/hit.wav")];

        let filled = fill_output(&layout, &output("Out", ""), &entries, None).expect("fill");
        let destinations: Vec<_> = filled.moves.iter().map(|(_, to)| to.clone()).collect();

        assert_eq!(
            destinations,
            vec![
                "Out/Media/art/logo.png".to_string(),
                "Out/Media/sfx/hit.wav".to_string()
            ]
        );
    }

    /// The counterpart: a glob with no folder in it names no subtree, so
    /// there is nothing to replace and the whole relative path is kept.
    #[test]
    fn a_glob_with_no_folder_in_it_strips_nothing() {
        let layout = Layout {
            name: "Out".to_string(),
            place: vec![Placement {
                from: Source::Matching("*.pdf".to_string()),
                into: "Docs".to_string(),
            }],
            ..Layout::default()
        };

        let filled = fill_output(
            &layout,
            &output("Out", ""),
            &[entry("deep/inside/manual.pdf")],
            None,
        )
        .expect("fill");

        assert_eq!(filled.moves[0].1, "Out/Docs/deep/inside/manual.pdf");
    }

    /// Detection runs inside one output's root, never across the input.
    /// Scored over everything at once these two payload folders tie, the
    /// tie-break picks one, and the other output would carry nothing.
    #[test]
    fn a_content_root_is_detected_inside_its_own_output() {
        let layout = Layout {
            name: "$mod_name".to_string(),
            place: vec![Placement {
                from: Source::ContentRoot,
                into: "Game".to_string(),
            }],
            ..Layout::default()
        };
        let entries = vec![
            entry("Mod A/readme.txt"),
            entry("Mod A/payload/Game.exe"),
            entry("Mod A/payload/data/pack.bin"),
            entry("Mod B/bundle/nw.exe"),
            entry("Mod B/bundle/www/index.html"),
        ];

        let first = fill_output(&layout, &output("A", "Mod A"), &entries, None).expect("fill");
        let second = fill_output(&layout, &output("B", "Mod B"), &entries, None).expect("fill");

        let destinations = |filled: &PlannedOutput| -> Vec<String> {
            filled.moves.iter().map(|(_, to)| to.clone()).collect()
        };
        assert_eq!(
            destinations(&first),
            vec![
                "A/Game/Game.exe".to_string(),
                "A/Game/data/pack.bin".to_string()
            ],
            "the first output's payload is found under its own root"
        );
        assert_eq!(
            destinations(&second),
            vec![
                "B/Game/nw.exe".to_string(),
                "B/Game/www/index.html".to_string()
            ],
            "and so is the second's"
        );
        assert!(
            first
                .moves
                .iter()
                .all(|(from, _)| from.starts_with("Mod A/")),
            "an output must not carry a sibling's files: {:?}",
            first.moves
        );
    }

    /// Two placements routing into one folder can send two same-named
    /// files to one path, and the applier would write one over the
    /// other. Nothing validates a filled output on its way out, so
    /// refusing here is the only thing between a layout that cannot mean
    /// what it says and a silently lost file.
    #[test]
    fn two_placements_landing_on_one_destination_are_refused() {
        let layout = Layout {
            place: vec![
                Placement {
                    from: Source::Matching("a/**".to_string()),
                    into: "X".to_string(),
                },
                Placement {
                    from: Source::Matching("b/**".to_string()),
                    into: "X".to_string(),
                },
            ],
            ..Layout::default()
        };
        let entries = vec![entry("a/f.bin"), entry("b/f.bin")];

        let error = fill_output(&layout, &output("Out", ""), &entries, None)
            .expect_err("one file would overwrite the other");
        assert!(
            format!("{error:#}").contains("Out/X/f.bin"),
            "the error must name the destination: {error:#}"
        );
    }

    /// Compared the way the execution boundary keys destinations, so a
    /// filesystem that folds case cannot merge two files this side of
    /// the check and lose one on the other side.
    #[test]
    fn two_destinations_a_filesystem_would_merge_are_refused() {
        let layout = Layout {
            place: vec![
                Placement {
                    from: Source::Matching("a/**".to_string()),
                    into: "X".to_string(),
                },
                Placement {
                    from: Source::Matching("b/**".to_string()),
                    into: "X".to_string(),
                },
            ],
            ..Layout::default()
        };
        let entries = vec![entry("a/File.bin"), entry("b/file.bin")];

        let error = fill_output(&layout, &output("Out", ""), &entries, None)
            .expect_err("a case-folding filesystem merges the two");
        let error = format!("{error:#}");
        assert!(error.contains("Out/X/File.bin"), "{error}");
        assert!(error.contains("Out/X/file.bin"), "{error}");
    }

    /// `$mod_name` is read out of a `modinfo.ini` inside the archive and
    /// substituted verbatim, so a hostile one reaches a destination. It
    /// is meant to: `CheckedRelativePath` refuses any `..` component and
    /// every execution path routes through it, which fails the whole
    /// plan loudly. A softer check here that skipped the one output
    /// would turn a traversal attempt into a shrug.
    #[test]
    fn a_name_that_climbs_out_reaches_the_boundary_that_refuses_it() {
        let layout = Layout {
            place: vec![Placement {
                from: Source::All,
                into: String::new(),
            }],
            ..Layout::default()
        };
        let escaping = ResolvedOutput {
            root: PathBuf::new(),
            name: "../evil".to_string(),
            variables: HashMap::new(),
        };

        let filled = fill_output(&layout, &escaping, &[entry("payload.bin")], None).expect("fill");

        assert_eq!(
            filled.moves[0].1, "../evil/payload.bin",
            "nothing here scrubs the name; the boundary refuses it"
        );
        assert!(
            crate::utilities::CheckedRelativePath::new(&filled.moves[0].1).is_err(),
            "a destination that climbs out must not pass validation"
        );
    }

    /// Most archives carry no indicator at all, so the line saying so is
    /// the one a preview shows most often.
    #[test]
    fn a_content_root_nothing_scored_says_so() {
        let layout = Layout {
            place: vec![Placement {
                from: Source::ContentRoot,
                into: "Game".to_string(),
            }],
            ..Layout::default()
        };
        let entries = vec![entry("notes/one.txt"), entry("notes/two.txt")];

        let filled = fill_output(&layout, &output("Out", ""), &entries, None).expect("fill");

        let reasoning = filled.reasoning.join("\n");
        assert!(
            reasoning.contains("nothing scored as a content root"),
            "{reasoning}"
        );
        assert!(reasoning.contains("notes"), "{reasoning}");
        assert_eq!(
            filled.moves[0].1, "Out/Game/one.txt",
            "the files still land, under the common prefix"
        );
    }

    /// A detection the preview cannot show its working for is one a user
    /// can only trust or distrust.
    #[test]
    fn a_content_root_placement_says_what_it_scored_on() {
        let layout = Layout {
            name: "Out".to_string(),
            place: vec![Placement {
                from: Source::ContentRoot,
                into: "Game".to_string(),
            }],
            ..Layout::default()
        };
        let entries = vec![
            entry("payload/Game.exe"),
            entry("payload/data/pack.bin"),
            entry("readme.txt"),
        ];

        let filled = fill_output(&layout, &output("Out", ""), &entries, None).expect("fill");
        let reasoning = filled.reasoning.join("\n");

        assert!(reasoning.contains("payload"), "{reasoning}");
        assert!(reasoning.contains("scored 3"), "{reasoning}");
        assert!(reasoning.contains("Game.exe"), "{reasoning}");
        assert!(reasoning.contains("an executable"), "{reasoning}");
        assert!(reasoning.contains("a data/ folder"), "{reasoning}");
    }

    /// `.` and the empty string both mean "the output's own root", and a
    /// `.` component is refused outright at the execution boundary — so
    /// a destination that names nothing has to collapse here.
    #[test]
    fn a_destination_that_names_nothing_collapses() {
        let layout = Layout {
            place: vec![Placement {
                from: Source::All,
                into: ".".to_string(),
            }],
            ..Layout::default()
        };

        let unwrapped =
            fill_output(&layout, &output("", ""), &[entry("sub/x.bin")], None).expect("fill");
        assert_eq!(unwrapped.moves[0].1, "sub/x.bin");

        let trailing = Layout {
            place: vec![Placement {
                from: Source::All,
                into: "Data/".to_string(),
            }],
            ..Layout::default()
        };
        let wrapped =
            fill_output(&trailing, &output("Out", ""), &[entry("sub/x.bin")], None).expect("fill");
        assert_eq!(wrapped.moves[0].1, "Out/Data/sub/x.bin");
    }

    /// An unresolved token costs an output its name, because a folder
    /// nobody can trace back to a mod is worse than a reported gap. A
    /// destination inside an output is not that: the files are still
    /// traceable, so the token is left standing where the user can see
    /// it and the reason is recorded rather than the output dropped.
    #[test]
    fn a_destination_template_that_will_not_resolve_is_left_standing_and_said_so() {
        let layout = Layout {
            place: vec![Placement {
                from: Source::All,
                into: "$version/data".to_string(),
            }],
            ..Layout::default()
        };

        let filled =
            fill_output(&layout, &output("Out", ""), &[entry("x.bin")], None).expect("fill");

        assert_eq!(filled.moves[0].1, "Out/$version/data/x.bin");
        let reasoning = filled.reasoning.join("\n");
        assert!(reasoning.contains("$version"), "{reasoning}");
        assert!(reasoning.contains("nothing set"), "{reasoning}");
    }

    /// The layered document carries the source-specific fields the
    /// extracted struct drops, so it wins whenever there is one.
    #[test]
    fn the_metadata_document_falls_back_to_the_struct_only_when_there_is_none() {
        let layout = Layout {
            generate: vec![Generated {
                into: "metadata.json".to_string(),
                content: GeneratedContent::MetadataDocument,
            }],
            ..Layout::default()
        };

        let layered = metadata(r#"{"maker_id":"RJ999001"}"#);
        let filled = fill_output(&layout, &output("Out", ""), &[], Some(&layered)).expect("fill");
        assert_eq!(
            filled.generated_files,
            vec![(
                "Out/metadata.json".to_string(),
                r#"{"maker_id":"RJ999001"}"#.to_string()
            )]
        );

        let bare = metadata("  ");
        let filled = fill_output(&layout, &output("Out", ""), &[], Some(&bare)).expect("fill");
        assert_eq!(filled.generated_files[0].0, "Out/metadata.json");
        assert!(
            filled.generated_files[0].1.contains("Placeholder Game"),
            "{:?}",
            filled.generated_files[0].1
        );

        let filled = fill_output(&layout, &output("Out", ""), &[], None).expect("fill");
        assert!(
            filled.generated_files.is_empty(),
            "there is no document to write when there is no metadata"
        );
    }

    fn screenshot_layout(name: &str) -> Layout {
        Layout {
            fetch: vec![Fetched {
                into: "screenshots".to_string(),
                source: FetchSource::Screenshots,
                name: name.to_string(),
            }],
            ..Layout::default()
        }
    }

    /// `$index` is a screenshot's position among all of them, not among
    /// the fetchable ones — the same numbering the cache keys use, so a
    /// file that cannot be fetched leaves a gap rather than a shift.
    /// The shipped `image_$index.$ext` reproduces the name the older
    /// vocabulary hardcoded, character for character.
    #[test]
    fn a_fetched_name_numbers_from_one_over_every_screenshot() {
        let layout = screenshot_layout("image_$index.$ext");
        let mut game = metadata("{}");
        game.screenshots = vec![
            ScreenshotData::Url("https://example.invalid/a.jpg".to_string()),
            ScreenshotData::FilePath(PathBuf::from("local/b.jpg")),
            ScreenshotData::Url("https://example.invalid/c.jpg".to_string()),
        ];

        let filled = fill_output(&layout, &output("Out", ""), &[], Some(&game)).expect("fill");

        assert_eq!(filled.downloads.len(), 2, "a local file is not fetchable");
        assert_eq!(
            filled.downloads[0].dest_path,
            "Out/screenshots/image_001.jpg"
        );
        assert_eq!(
            filled.downloads[0].cache_key,
            "dlsite:RJ123456:screenshot_0"
        );
        assert_eq!(filled.downloads[0].product_id.as_deref(), Some("RJ123456"));
        assert_eq!(
            filled.downloads[1].dest_path,
            "Out/screenshots/image_003.jpg"
        );
        assert_eq!(
            filled.downloads[1].cache_key,
            "dlsite:RJ123456:screenshot_2"
        );
    }

    /// Unpadded, ten screenshots sort 1, 10, 2 in every file browser
    /// there is. Three digits is what the older vocabulary wrote and
    /// what `$index` has to keep writing.
    #[test]
    fn a_fetched_index_is_padded_so_ten_of_them_still_sort() {
        let layout = screenshot_layout("image_$index.$ext");
        let mut game = metadata("{}");
        game.screenshots = (0..10)
            .map(|_| ScreenshotData::Url("https://example.invalid/a.jpg".to_string()))
            .collect();

        let filled = fill_output(&layout, &output("Out", ""), &[], Some(&game)).expect("fill");

        let names: Vec<String> = filled
            .downloads
            .iter()
            .map(|download| download.dest_path.clone())
            .collect();
        assert_eq!(names.len(), 10);
        assert_eq!(names[0], "Out/screenshots/image_001.jpg");
        assert_eq!(names[9], "Out/screenshots/image_010.jpg");
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted, names, "fetch order and sort order are the same");
    }

    /// A template that spells an extension out renames whatever the
    /// source actually serves. `$ext` exists so it does not have to.
    #[test]
    fn a_fetched_extension_follows_its_source_url() {
        let layout = screenshot_layout("image_$index.$ext");
        let mut game = metadata("{}");
        game.screenshots = vec![
            ScreenshotData::Url("https://example.invalid/shot.png".to_string()),
            ScreenshotData::Url("https://example.invalid/no-extension".to_string()),
        ];

        let filled = fill_output(&layout, &output("Out", ""), &[], Some(&game)).expect("fill");

        assert_eq!(
            filled.downloads[0].dest_path,
            "Out/screenshots/image_001.png"
        );
        assert_eq!(
            filled.downloads[1].dest_path, "Out/screenshots/image_002.jpg",
            "a URL naming no extension still gets a plausible one"
        );
    }

    /// A file no placement wanted is dropped on purpose, and a drop
    /// nobody is told about is indistinguishable from a bug. A count
    /// alone is not enough: told "47 files were not carried", a user
    /// cannot tell whether those were the right 47.
    #[test]
    fn an_output_names_the_files_it_left_behind() {
        let layout = Layout {
            place: vec![Placement {
                from: Source::Matching("keep/**".to_string()),
                into: String::new(),
            }],
            ..Layout::default()
        };
        let entries = vec![
            entry("keep/wanted.bin"),
            entry("drop/one.bin"),
            entry("drop/two.bin"),
        ];

        let filled = fill_output(&layout, &output("Out", ""), &entries, None).expect("fill");

        let left = filled
            .reasoning
            .iter()
            .find(|line| line.contains("no placement"))
            .unwrap_or_else(|| panic!("{:?}", filled.reasoning));
        assert!(left.contains("2 files"), "{left}");
        assert!(left.contains("drop/one.bin"), "{left}");
        assert!(left.contains("drop/two.bin"), "{left}");
    }

    /// The listing is capped, because a rule that matches nothing leaves
    /// the whole archive behind and a preview is not a place to print
    /// ten thousand paths.
    #[test]
    fn a_long_list_of_left_behind_files_is_cut_short() {
        let layout = Layout {
            place: vec![Placement {
                from: Source::Matching("keep/**".to_string()),
                into: String::new(),
            }],
            ..Layout::default()
        };
        let entries: Vec<_> = (0..7).map(|n| entry(&format!("drop/{n}.bin"))).collect();

        let filled = fill_output(&layout, &output("Out", ""), &entries, None).expect("fill");

        let left = filled
            .reasoning
            .iter()
            .find(|line| line.contains("no placement"))
            .unwrap_or_else(|| panic!("{:?}", filled.reasoning));
        assert!(left.contains("7 files"), "{left}");
        assert!(left.contains("drop/0.bin"), "{left}");
        assert!(left.contains("and 4 more"), "{left}");
        assert!(!left.contains("drop/6.bin"), "{left}");
    }

    /// The layout that will produce the most outputs infers nothing at
    /// all: one `All` placement, a name that resolves, nothing left
    /// over. A preview with no explanation on it is the explainability
    /// property failing exactly where it matters most, so what the
    /// layout did is recorded even when there was nothing to work out.
    #[test]
    fn an_output_that_inferred_nothing_still_says_what_it_did() {
        let layout = Layout {
            name: "$mod_name".to_string(),
            place: vec![Placement {
                from: Source::All,
                into: String::new(),
            }],
            ..Layout::default()
        };
        let entries = vec![
            entry("Placeholder Mod/one.bin"),
            entry("Placeholder Mod/two.bin"),
        ];

        let filled = fill_output(
            &layout,
            &output("Placeholder Mod", "Placeholder Mod"),
            &entries,
            None,
        )
        .expect("fill");

        assert_eq!(filled.moves.len(), 2);
        assert_eq!(
            filled.reasoning,
            vec![
                "everything under the output's root placed 2 files into Placeholder Mod"
                    .to_string()
            ]
        );
    }

    /// A placement that matched nothing is the quiet failure a glob
    /// makes easiest, so it says so rather than leaving a gap.
    #[test]
    fn a_placement_that_matched_nothing_says_so() {
        let layout = Layout {
            place: vec![Placement {
                from: Source::Matching("absent/**".to_string()),
                into: "Docs".to_string(),
            }],
            ..Layout::default()
        };

        let filled =
            fill_output(&layout, &output("Out", ""), &[entry("a.bin")], None).expect("fill");

        assert!(
            filled
                .reasoning
                .iter()
                .any(|line| line.contains("absent/**") && line.contains("placed nothing")),
            "{:?}",
            filled.reasoning
        );
    }

    /// Reasoning is read, not counted. The same sentence twice tells a
    /// reader nothing the first one did not.
    #[test]
    fn one_reason_is_not_repeated() {
        let layout = Layout {
            place: vec![
                Placement {
                    from: Source::Matching("a/**".to_string()),
                    into: "$version".to_string(),
                },
                Placement {
                    from: Source::Matching("b/**".to_string()),
                    into: "$version".to_string(),
                },
            ],
            ..Layout::default()
        };
        let entries = vec![entry("a/one.bin"), entry("b/two.bin")];

        let filled = fill_output(&layout, &output("Out", ""), &entries, None).expect("fill");

        assert_eq!(
            filled
                .reasoning
                .iter()
                .filter(|line| line.contains("nothing set"))
                .count(),
            1,
            "{:?}",
            filled.reasoning
        );
    }
}
