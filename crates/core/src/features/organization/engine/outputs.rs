//! Resolve a layout to its named outputs.
//!
//! The first half of resolving a layout: how many outputs an archive
//! produces, where each one's content starts inside the input, and what
//! each one is called. What goes *into* each output is the other half,
//! and reads the [`ResolvedOutput`]s this half returns.
//!
//! Two rules run through the whole file. An output that cannot be named
//! is reported on [`ResolvedOutputs::skipped`] and dropped, never named
//! after something else — a folder nobody can trace back to a mod is
//! worse than one the user is told went missing. And nothing here may
//! depend on a `HashMap`'s iteration order: roots are sorted, and
//! templates drive their own expansion.

use crate::archive::ArchiveEntry;
use crate::features::conversion::modinfo::{self, ModInfo};
use crate::features::organization::layout::{FileVariable, Layout, OutputSelector};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

/// One output the layout produces, located and named.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedOutput {
    /// Where this output's content starts in the input. Empty for `Whole`.
    pub root: PathBuf,
    /// The resolved folder name. Empty means no wrapper.
    pub name: String,
    /// Variables usable in this output's `into` templates. Naming an
    /// output does not read them back — expanding an `into` against
    /// them is the other half of resolution.
    pub variables: HashMap<String, String>,
}

/// Every output a layout resolved to, and every one it could not.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedOutputs {
    pub outputs: Vec<ResolvedOutput>,
    /// One `(root, reason)` per output that could not be named.
    pub skipped: Vec<(String, String)>,
}

/// Work out how many outputs `layout` produces from `entries`, where
/// each one's content starts, and what each is called.
///
/// `read_entry` is handed the archive-relative path of a file variable's
/// file and returns its bytes. Only the files the layout actually names
/// are read: planning must not pull a payload out of the archive to
/// decide a folder name.
///
/// Errors are layout faults, not archive faults — a key no
/// `modinfo.ini` carries, two outputs claiming one folder name, several
/// outputs with no wrapper folder to keep them apart. An archive that
/// simply does not answer a question the layout asked costs that one
/// output, which lands on `skipped` while its siblings resolve.
pub(crate) fn resolve_outputs(
    layout: &Layout,
    entries: &[ArchiveEntry],
    base_variables: &HashMap<String, String>,
    read_entry: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<ResolvedOutputs> {
    // A key no modinfo.ini can carry is a mistake in the layout, not a
    // property of this archive. Refusing it here rather than letting it
    // read as an empty variable means the same layout fails the same
    // way on every input, before anything is read.
    let mut wanted = Vec::with_capacity(layout.file_variables.len());
    for variable in &layout.file_variables {
        let Some(key) = ModInfoKey::parse(&variable.key) else {
            bail!(
                "file variable ${} asks a modinfo.ini for {:?}, which is not one of its keys \
                 (name, addonfor, screenshot)",
                variable.as_name,
                variable.key
            );
        };
        wanted.push((variable, key));
    }

    let roots = select_roots(&layout.outputs, entries);

    // An empty name means the output has no wrapper folder, so several
    // outputs sharing one would all unpack into the same place. That is
    // broken whatever the archive holds; say so before reading a byte.
    if layout.name.is_empty() && roots.len() > 1 {
        bail!(
            "{} outputs were selected but the layout has no name: with no wrapper folder \
             around each one they would all unpack into the same place",
            roots.len()
        );
    }

    let mut outputs = Vec::new();
    let mut skipped = Vec::new();
    for root in roots {
        match resolve_one(&layout.name, &wanted, &root, base_variables, read_entry) {
            Resolution::Named(output) => outputs.push(output),
            Resolution::Unnameable(reason) => skipped.push((root, reason)),
        }
    }

    // The same wrapper rule again, for a name template that clears the
    // check above and then expands to nothing.
    if outputs.len() > 1 && outputs.iter().any(|output| output.name.is_empty()) {
        bail!(
            "{} outputs resolved but one of them is unnamed: without a wrapper folder it would \
             unpack over its siblings",
            outputs.len()
        );
    }

    // Two outputs claiming one name is two mods unpacking into one
    // folder, with the second silently winning. Compared through
    // `to_uppercase`, which is how `OrganizationPlan::validate_paths`
    // keys destinations. That is nobody's filesystem table — it folds ß
    // to SS and ς to Σ where NTFS does not — so it refuses a little
    // more than any one filesystem would merge. That is the right side
    // to err on: refusing a separable pair costs a message, merging two
    // mods into one folder costs the contents of one of them.
    // `claimed` is only ever inserted into and looked up, never
    // iterated, so it cannot reorder the result.
    let mut claimed: BTreeMap<String, String> = BTreeMap::new();
    for output in &outputs {
        if let Some(first) = claimed.insert(output.name.to_uppercase(), output.name.clone()) {
            if first == output.name {
                bail!("two outputs resolved to the same name: {:?}", output.name);
            }
            bail!(
                "two outputs resolved to names a filesystem may merge: {first:?} and {:?}",
                output.name
            );
        }
    }

    Ok(ResolvedOutputs { outputs, skipped })
}

/// Whether an output could be named.
enum Resolution {
    Named(ResolvedOutput),
    /// Why this output has no name, for the caller to report.
    Unnameable(String),
}

/// The keys a file variable may take from a `modinfo.ini`. Parsed once
/// per layout so the valid set is written down here only, and the read
/// below has no unreachable arm to fall through.
#[derive(Debug, Clone, Copy)]
enum ModInfoKey {
    Name,
    AddonFor,
    Screenshot,
}

impl ModInfoKey {
    fn parse(key: &str) -> Option<Self> {
        match key {
            "name" => Some(Self::Name),
            "addonfor" => Some(Self::AddonFor),
            "screenshot" => Some(Self::Screenshot),
            _ => None,
        }
    }

    fn take(self, info: ModInfo) -> Option<String> {
        match self {
            Self::Name => info.name,
            Self::AddonFor => info.addonfor,
            Self::Screenshot => info.screenshot,
        }
    }
}

/// The directories the selector picks out, lexicographically ordered so
/// one archive resolves to one output order on every run.
fn select_roots(selector: &OutputSelector, entries: &[ArchiveEntry]) -> Vec<String> {
    match selector {
        OutputSelector::Whole => vec![String::new()],
        OutputSelector::PerDirectoryContaining { marker } => {
            let mut roots = BTreeSet::new();
            for entry in entries {
                // The marker is a file. A directory that happens to be
                // called `modinfo.ini` names nothing.
                if entry.is_dir {
                    continue;
                }
                let path = entry.path.replace('\\', "/");
                if path == *marker {
                    // A marker at the top level makes the archive itself
                    // the one output, rooted where it already sits.
                    roots.insert(String::new());
                } else if let Some(root) = path
                    .strip_suffix(marker.as_str())
                    .and_then(|root| root.strip_suffix('/'))
                {
                    roots.insert(root.to_string());
                }
            }
            roots.into_iter().collect()
        }
    }
}

/// Resolve one output's file variables and name.
fn resolve_one(
    name_template: &str,
    wanted: &[(&FileVariable, ModInfoKey)],
    root: &str,
    base_variables: &HashMap<String, String>,
    read_entry: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Resolution {
    let mut variables = base_variables.clone();
    let mut unresolved = Vec::new();

    for (variable, key) in wanted {
        // A file variable's `file` is relative to the output's own root,
        // so two outputs read two different files through one layout.
        let path = join(root, &variable.file);
        match read_file_variable(&path, variable, *key, read_entry) {
            Ok(value) => {
                variables.insert(variable.as_name.clone(), value);
            }
            Err(reason) => unresolved.push(reason),
        }
    }

    let (name, missing) = expand(name_template, &variables);

    // A layout declares a file variable in order to use it, so one that
    // did not resolve costs this output — reported, with the reason the
    // archive gave, rather than papered over with a fallback name.
    if !unresolved.is_empty() {
        return Resolution::Unnameable(unresolved.join("; "));
    }
    if !missing.is_empty() {
        let tokens: Vec<String> = missing.iter().map(|token| format!("${token}")).collect();
        return Resolution::Unnameable(format!(
            "the name needs {}, which nothing set",
            tokens.join(", ")
        ));
    }

    Resolution::Named(ResolvedOutput {
        root: PathBuf::from(root),
        name,
        variables,
    })
}

/// Read one file variable, or say why it has no value.
fn read_file_variable(
    path: &str,
    variable: &FileVariable,
    key: ModInfoKey,
    read_entry: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> std::result::Result<String, String> {
    let Some(bytes) = read_entry(path) else {
        return Err(format!(
            "${} comes from {path}, which the archive does not hold",
            variable.as_name
        ));
    };
    // Decoded strictly, as the on-disk parser does. Substituting
    // replacement characters would turn a Shift-JIS name into a folder
    // of question marks, which is a name nobody chose.
    let Ok(text) = String::from_utf8(bytes) else {
        return Err(format!(
            "${} comes from {path}, which is not valid UTF-8",
            variable.as_name
        ));
    };
    key.take(modinfo::parse_str(&text)).ok_or_else(|| {
        format!(
            "${} comes from the {} of {path}, which does not set one",
            variable.as_name, variable.key
        )
    })
}

/// Join a path inside the archive. Archive entry paths are
/// `/`-separated strings whatever the host is, and `PathBuf::join`
/// would write a `\` into one on Windows.
fn join(root: &str, relative: &str) -> String {
    if root.is_empty() {
        relative.to_string()
    } else {
        format!("{root}/{relative}")
    }
}

/// Expand `$token`s in `template`, returning the result and every token
/// that had no value, in order of first appearance.
///
/// One left-to-right pass driven by the template rather than by the
/// variable map, which buys three things: a `HashMap`'s iteration order
/// cannot decide the result, `$mod` cannot eat the front of `$mod_name`,
/// and a value that itself contains a `$` is not rescanned as a token.
/// An unresolved token is left standing so a caller that ignores the
/// second return value gets a visibly broken name, not a plausible one.
pub(super) fn expand(template: &str, variables: &HashMap<String, String>) -> (String, Vec<String>) {
    let mut expanded = String::with_capacity(template.len());
    let mut missing: Vec<String> = Vec::new();
    let mut rest = template;

    while let Some(at) = rest.find('$') {
        expanded.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let end = after
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(after.len());
        let token = &after[..end];

        if token.is_empty() {
            // A lone `$` is a literal, not an unresolved token.
            expanded.push('$');
        } else if let Some(value) = variables.get(token) {
            expanded.push_str(value);
        } else {
            expanded.push('$');
            expanded.push_str(token);
            if !missing.iter().any(|seen| seen == token) {
                missing.push(token.to_string());
            }
        }

        rest = &after[end..];
    }
    expanded.push_str(rest);

    (expanded, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::ArchiveEntry;
    use crate::features::organization::layout::{FileVariable, Layout, OutputSelector};

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

    fn no_reads(_: &str) -> Option<Vec<u8>> {
        None
    }

    #[test]
    fn whole_is_one_output_rooted_at_the_top() {
        let layout = Layout {
            name: "$title".to_string(),
            ..Layout::default()
        };
        let mut base = HashMap::new();
        base.insert("title".to_string(), "Placeholder Game".to_string());

        let resolved = resolve_outputs(&layout, &[entry("anything/at/all.bin")], &base, &no_reads)
            .expect("resolve");

        assert_eq!(resolved.outputs.len(), 1);
        assert_eq!(resolved.outputs[0].root, PathBuf::new());
        assert_eq!(resolved.outputs[0].name, "Placeholder Game");
        assert!(resolved.skipped.is_empty());
    }

    #[test]
    fn a_marker_makes_one_output_per_folder_named_from_that_marker() {
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![FileVariable {
                as_name: "mod_name".to_string(),
                file: "modinfo.ini".to_string(),
                key: "name".to_string(),
            }],
            name: "$mod_name".to_string(),
            ..Layout::default()
        };
        // Each folder's name sorts against its root, so the assertions
        // below say which of the two the order follows. Listed with the
        // later root first, so input order is a third answer.
        let entries = vec![
            entry("Variant Zulu/modinfo.ini"),
            entry("Variant Zulu/natives/body.pak"),
            entry("Variant Alpha/modinfo.ini"),
            entry("Variant Alpha/natives/body.pak"),
        ];
        let read = |path: &str| -> Option<Vec<u8>> {
            match path {
                "Variant Zulu/modinfo.ini" => Some(b"name=Anvil Mod".to_vec()),
                "Variant Alpha/modinfo.ini" => Some(b"name=Zebra Mod".to_vec()),
                _ => None,
            }
        };

        let resolved = resolve_outputs(&layout, &entries, &HashMap::new(), &read).expect("resolve");

        let roots: Vec<_> = resolved.outputs.iter().map(|o| o.root.clone()).collect();
        assert_eq!(
            roots,
            vec![
                PathBuf::from("Variant Alpha"),
                PathBuf::from("Variant Zulu")
            ],
            "outputs come out in root order"
        );
        let names: Vec<_> = resolved.outputs.iter().map(|o| o.name.clone()).collect();
        assert_eq!(
            names,
            vec!["Zebra Mod".to_string(), "Anvil Mod".to_string()],
            "each folder is named from its own marker, and the names do not sort"
        );
        assert!(resolved.skipped.is_empty());
    }

    #[test]
    fn an_output_whose_name_will_not_resolve_is_skipped_and_its_siblings_survive() {
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![FileVariable {
                as_name: "mod_name".to_string(),
                file: "modinfo.ini".to_string(),
                key: "name".to_string(),
            }],
            name: "$mod_name".to_string(),
            ..Layout::default()
        };
        let entries = vec![entry("Good/modinfo.ini"), entry("Nameless/modinfo.ini")];
        let read = |path: &str| -> Option<Vec<u8>> {
            match path {
                "Good/modinfo.ini" => Some(b"name=Good Mod".to_vec()),
                // Present, but carries no name key.
                "Nameless/modinfo.ini" => Some(b"addonfor=Something".to_vec()),
                _ => None,
            }
        };

        let resolved = resolve_outputs(&layout, &entries, &HashMap::new(), &read).expect("resolve");

        assert_eq!(resolved.outputs.len(), 1);
        assert_eq!(resolved.outputs[0].name, "Good Mod");
        assert_eq!(resolved.skipped.len(), 1);
        assert_eq!(resolved.skipped[0].0, "Nameless");
    }

    #[test]
    fn two_outputs_resolving_to_one_name_are_refused() {
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![FileVariable {
                as_name: "mod_name".to_string(),
                file: "modinfo.ini".to_string(),
                key: "name".to_string(),
            }],
            name: "$mod_name".to_string(),
            ..Layout::default()
        };
        let entries = vec![entry("One/modinfo.ini"), entry("Two/modinfo.ini")];
        let read = |_: &str| Some(b"name=Same Name".to_vec());

        let error = resolve_outputs(&layout, &entries, &HashMap::new(), &read)
            .expect_err("two outputs cannot share a root name");
        assert!(
            format!("{error:#}").contains("Same Name"),
            "the error must name the collision: {error:#}"
        );
    }

    #[test]
    fn planning_reads_only_the_entries_a_layout_names() {
        use std::cell::RefCell;

        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![FileVariable {
                as_name: "mod_name".to_string(),
                file: "modinfo.ini".to_string(),
                key: "name".to_string(),
            }],
            name: "$mod_name".to_string(),
            ..Layout::default()
        };
        let entries = vec![
            entry("Only/modinfo.ini"),
            entry("Only/natives/huge.pak"),
            entry("Only/readme.txt"),
        ];
        let asked: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let read = |path: &str| -> Option<Vec<u8>> {
            asked.borrow_mut().push(path.to_string());
            Some(b"name=Only Mod".to_vec())
        };

        resolve_outputs(&layout, &entries, &HashMap::new(), &read).expect("resolve");

        assert_eq!(
            asked.into_inner(),
            vec!["Only/modinfo.ini".to_string()],
            "planning must not read entries no layout asked for"
        );
    }

    #[test]
    fn a_marker_selector_with_an_empty_name_is_refused() {
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            name: String::new(),
            ..Layout::default()
        };
        let entries = vec![entry("One/modinfo.ini"), entry("Two/modinfo.ini")];

        let error = resolve_outputs(&layout, &entries, &HashMap::new(), &no_reads)
            .expect_err("several unwrapped outputs collide by construction");
        assert!(format!("{error:#}").contains("wrapper"), "{error:#}");
    }

    /// A skipped output is only useful if its reason says what to go and
    /// look at. An empty string satisfies "there is a reason".
    #[test]
    fn every_skipped_output_says_which_variable_failed_and_where() {
        // The variable is called `preview` and takes the `screenshot`
        // key, so a reason naming both names two different things.
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![FileVariable {
                as_name: "preview".to_string(),
                file: "modinfo.ini".to_string(),
                key: "screenshot".to_string(),
            }],
            name: "$preview".to_string(),
            ..Layout::default()
        };
        let entries = vec![entry("Absent/modinfo.ini"), entry("Nameless/modinfo.ini")];
        let read = |path: &str| -> Option<Vec<u8>> {
            match path {
                // Listed in the archive, but nothing comes back.
                "Absent/modinfo.ini" => None,
                // Read, parsed, and it sets no screenshot.
                _ => Some(b"name=Placeholder Mod".to_vec()),
            }
        };

        let resolved = resolve_outputs(&layout, &entries, &HashMap::new(), &read).expect("resolve");

        assert!(resolved.outputs.is_empty());
        assert_eq!(resolved.skipped.len(), 2);

        let (root, reason) = &resolved.skipped[0];
        assert_eq!(root, "Absent");
        assert!(reason.contains("$preview"), "{reason}");
        assert!(reason.contains("Absent/modinfo.ini"), "{reason}");

        let (root, reason) = &resolved.skipped[1];
        assert_eq!(root, "Nameless");
        assert!(reason.contains("$preview"), "{reason}");
        assert!(reason.contains("Nameless/modinfo.ini"), "{reason}");
        assert!(reason.contains("screenshot"), "{reason}");
    }

    /// Names are compared case-insensitively, because a filesystem that
    /// folds case merges the two folders and the second mod silently
    /// wins. Nothing else covers that branch.
    #[test]
    fn two_outputs_whose_names_differ_only_in_case_are_refused() {
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![FileVariable {
                as_name: "mod_name".to_string(),
                file: "modinfo.ini".to_string(),
                key: "name".to_string(),
            }],
            name: "$mod_name".to_string(),
            ..Layout::default()
        };
        let entries = vec![entry("Lower/modinfo.ini"), entry("Upper/modinfo.ini")];
        let read = |path: &str| -> Option<Vec<u8>> {
            match path {
                "Lower/modinfo.ini" => Some(b"name=placeholder mod".to_vec()),
                "Upper/modinfo.ini" => Some(b"name=Placeholder Mod".to_vec()),
                _ => None,
            }
        };

        let error = resolve_outputs(&layout, &entries, &HashMap::new(), &read)
            .expect_err("a filesystem that folds case merges the two");
        let error = format!("{error:#}");
        assert!(error.contains("placeholder mod"), "{error}");
        assert!(error.contains("Placeholder Mod"), "{error}");
    }

    /// A key no modinfo.ini carries is a mistake in the layout. Reading
    /// it as an empty variable would turn that into a skipped output on
    /// every archive, which blames the wrong thing.
    #[test]
    fn a_file_variable_asking_for_a_key_a_modinfo_has_no_such_thing_as_is_refused() {
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![FileVariable {
                as_name: "author".to_string(),
                file: "modinfo.ini".to_string(),
                key: "author".to_string(),
            }],
            name: "$author".to_string(),
            ..Layout::default()
        };
        let entries = vec![entry("One/modinfo.ini")];

        // `no_reads` would make any variable unresolvable, so a skipped
        // output rather than an error would mean the key went unchecked.
        let error = resolve_outputs(&layout, &entries, &HashMap::new(), &no_reads)
            .expect_err("a modinfo.ini has no author key");
        let error = format!("{error:#}");
        assert!(error.contains("author"), "{error}");
        assert!(
            error.contains("addonfor"),
            "the error should say which keys there are: {error}"
        );
    }

    /// Expanding by walking the variable map and calling `replace` per
    /// key lets `$mod` rewrite the front of `$mod_name` whenever the map
    /// yields `mod` first. A `HashMap`'s order is not fixed, so that bug
    /// hides about half the time — repeat, in both insertion orders, so
    /// it cannot. This is why the expander here walks the template.
    #[test]
    fn a_variable_whose_name_prefixes_another_does_not_eat_it() {
        let layout = Layout {
            name: "$mod_name for $mod".to_string(),
            ..Layout::default()
        };

        for _ in 0..32 {
            for order in [["mod", "mod_name"], ["mod_name", "mod"]] {
                let mut base = HashMap::new();
                for key in order {
                    let value = if key == "mod" {
                        "Placeholder Game"
                    } else {
                        "Placeholder Mod"
                    };
                    base.insert(key.to_string(), value.to_string());
                }

                let resolved = resolve_outputs(&layout, &[], &base, &no_reads).expect("resolve");

                assert_eq!(
                    resolved.outputs[0].name, "Placeholder Mod for Placeholder Game",
                    "inserted {order:?} first"
                );
            }
        }
    }

    /// The wrapper rule is checked twice: once on the template, which
    /// costs nothing, and once on what the template resolved to, which
    /// is the only place this case shows up.
    #[test]
    fn a_name_that_resolves_to_nothing_leaves_several_outputs_unwrapped() {
        // A template with something in it, so the check on the template
        // itself passes and only the check on the result can fire.
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            name: "$suffix".to_string(),
            ..Layout::default()
        };
        let mut base = HashMap::new();
        base.insert("suffix".to_string(), String::new());
        let entries = vec![entry("One/modinfo.ini"), entry("Two/modinfo.ini")];

        let error = resolve_outputs(&layout, &entries, &base, &no_reads)
            .expect_err("a name that resolves to nothing wraps nothing");
        assert!(format!("{error:#}").contains("wrapper"), "{error:#}");
    }

    /// The marker names a file. A directory called `modinfo.ini` would
    /// otherwise select an output that then had nothing to read.
    #[test]
    fn a_directory_named_like_the_marker_selects_nothing() {
        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![FileVariable {
                as_name: "mod_name".to_string(),
                file: "modinfo.ini".to_string(),
                key: "name".to_string(),
            }],
            name: "$mod_name".to_string(),
            ..Layout::default()
        };
        let entries = vec![
            ArchiveEntry {
                path: "Decoy/modinfo.ini".to_string(),
                size: 0,
                packed_size: 0,
                modified: None,
                is_dir: true,
                encrypted: false,
                crc32: None,
            },
            entry("Real/modinfo.ini"),
        ];
        let read = |path: &str| -> Option<Vec<u8>> {
            match path {
                "Real/modinfo.ini" => Some(b"name=Placeholder Mod".to_vec()),
                _ => None,
            }
        };

        let resolved = resolve_outputs(&layout, &entries, &HashMap::new(), &read).expect("resolve");

        assert_eq!(resolved.outputs.len(), 1);
        assert_eq!(resolved.outputs[0].root, PathBuf::from("Real"));
        assert!(
            resolved.skipped.is_empty(),
            "the decoy must not be selected at all, not selected and then skipped: {:?}",
            resolved.skipped
        );
    }

    /// An archive that is itself one mod carries the marker at its top
    /// level, and the output is rooted where the content already sits.
    #[test]
    fn a_marker_at_the_top_level_makes_the_input_itself_one_output() {
        use std::cell::RefCell;

        let layout = Layout {
            outputs: OutputSelector::PerDirectoryContaining {
                marker: "modinfo.ini".to_string(),
            },
            file_variables: vec![FileVariable {
                as_name: "mod_name".to_string(),
                file: "modinfo.ini".to_string(),
                key: "name".to_string(),
            }],
            name: "$mod_name".to_string(),
            ..Layout::default()
        };
        let entries = vec![entry("modinfo.ini"), entry("natives/body.pak")];
        let asked: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let read = |path: &str| -> Option<Vec<u8>> {
            asked.borrow_mut().push(path.to_string());
            Some(b"name=Placeholder Mod".to_vec())
        };

        let resolved = resolve_outputs(&layout, &entries, &HashMap::new(), &read).expect("resolve");

        assert_eq!(resolved.outputs.len(), 1);
        assert_eq!(resolved.outputs[0].root, PathBuf::new());
        assert_eq!(resolved.outputs[0].name, "Placeholder Mod");
        assert_eq!(
            asked.into_inner(),
            vec!["modinfo.ini".to_string()],
            "a file variable of an output rooted at the top carries no leading separator"
        );
    }
}
