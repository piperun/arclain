pub mod engine;
pub mod flatten_helper;
pub mod layout;
pub mod metrics;
pub mod organizer;
pub mod presets;
pub mod profile;
#[cfg(test)]
pub mod pruning_tests;
pub mod session;

pub mod checks;
pub mod downloads;
pub mod flatten;
pub mod metadata;
pub mod tasks;

pub use checks::*;
pub use metadata::{GameMetadata, ScreenshotData};
pub use organizer::*;
pub use profile::{list_archive_profiles, load_archive_profile, ArchiveFormat, ArchiveProfile};

use crate::features::organization::layout::{
    FetchSource, Fetched, Generated, GeneratedContent, Layout, OutputSelector, Placement, Source,
};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrganizationRule {
    /// Database ID (0 for new rules)
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub priority: i32,
    pub is_enabled: bool,
    pub trigger: RuleTrigger,
    pub actions: RuleActions,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleTrigger {
    pub metadata_source: Option<String>,
    pub filename_pattern: Option<String>,
    pub has_file: Option<String>,
}

/// What a matched rule does. Everything about the *shape* of the result
/// lives in `layout`; `output_name` is deliberately outside it, because
/// it names the container rather than the arrangement inside it and a
/// run writing a folder rather than an archive ignores it.
#[derive(Debug, Clone, Serialize, Default)]
pub struct RuleActions {
    /// Template for output archive name (uses template variables)
    #[serde(default)]
    pub output_name: Option<String>,
    pub layout: Layout,
}

/// Rules are stored as a serialized `actions_json` blob rather than as
/// normalized columns, so a saved rule from before layouts were data
/// arrives here with `root_folder`, `move_files` and
/// `use_standard_layout` and no `layout` at all. Translate on read: a
/// document carrying `layout` is current, and anything else is read
/// through the old vocabulary and converted. No schema migration is
/// involved, and nobody's library reorganizes differently for having
/// been saved before the change.
impl<'de> Deserialize<'de> for RuleActions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let document = RuleActionsDocument::deserialize(deserializer)?;
        let layout = match document.layout {
            Some(layout) => layout,
            None => layout_from_legacy_actions(
                document.root_folder,
                document.move_files,
                document.use_standard_layout,
            ),
        };
        Ok(RuleActions {
            output_name: document.output_name,
            layout,
        })
    }
}

/// Every field either vocabulary may carry, all optional, so one read
/// accepts both shapes without asking the caller which it has.
#[derive(Deserialize)]
struct RuleActionsDocument {
    #[serde(default)]
    output_name: Option<String>,
    #[serde(default)]
    layout: Option<Layout>,
    #[serde(default)]
    root_folder: Option<String>,
    #[serde(default)]
    move_files: Vec<MoveAction>,
    #[serde(default)]
    use_standard_layout: bool,
}

/// `use_standard_layout: true` was one boolean standing for a whole
/// layout: detect the content root, put it under `Game/`, write the
/// metadata document, fetch the screenshots. Written out, it is this.
///
/// The two branches disagree about the capitalisation of the screenshot
/// folder, and that is preserved rather than tidied: the boolean path
/// fetched into `screenshots` and the explicit path into `Screenshots`,
/// so making them agree would move files for one existing rule or the
/// other.
///
/// Three things a rule saved under the old vocabulary used to do are
/// *not* reproduced. None of them can be said in the layout vocabulary,
/// and each is a visible change to a rule someone may have saved, so
/// they are written down here rather than left to be discovered.
///
/// **` v$version` no longer strips itself.** The retired expander
/// special-cased that exact string: a template of `"$title v$version"`
/// became `"Title"` when nothing knew the version, degrading to a
/// slightly awkward folder name. Expansion is template-driven now and
/// reports what it could not fill, and an unresolved token in an
/// output's *name* costs that output — so the same rule on the same
/// archive produces no folder at all, with `$version` named in the
/// reason on `OrganizationPlan::skipped_outputs`. That reason is the
/// whole difference between a loss a user can read and a silent one. A
/// layout wanting a fallback writes one into its template.
///
/// **A `move_files` rule keeps its wrapper folder.** The retired code
/// stripped the entries' longest common path prefix before appending
/// the target, so `wrapper/Game.exe` with a target of `bin` landed at
/// `Out/bin/Game.exe`; it now lands at `Out/bin/wrapper/Game.exe`,
/// because a placement strips only what its own glob spells out. The
/// prefix is a property of the archive rather than of the layout, which
/// is why no field can carry it, and stripping a wrapper is what a
/// `ContentRoot` placement is for.
///
/// **A file no pattern matched is not carried.** It used to fall
/// through to a `game/` folder. Placements claim files and a file
/// nothing claimed stays behind, which the output's `reasoning` says by
/// naming the paths rather than leaving it silent.
pub fn layout_from_legacy_actions(
    root_folder: Option<String>,
    move_files: Vec<MoveAction>,
    use_standard_layout: bool,
) -> Layout {
    // `create_plan` read an absent `root_folder` as the literal "Game",
    // so an old rule that never set one still produced a wrapper.
    let name = root_folder.unwrap_or_else(|| "Game".to_string());
    if use_standard_layout {
        return Layout {
            outputs: OutputSelector::Whole,
            file_variables: vec![],
            name,
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
                // Reproduces `image_{:03}.{ext}` exactly: `$index` is
                // padded to three digits and `$ext` comes from the
                // source URL, so a `.png` is not renamed to `.jpg`.
                name: "image_$index.$ext".to_string(),
            }],
        };
    }
    Layout {
        outputs: OutputSelector::Whole,
        file_variables: vec![],
        name,
        place: move_files
            .into_iter()
            .map(|action| Placement {
                from: Source::Matching(action.pattern),
                into: action.target,
            })
            .collect(),
        generate: vec![Generated {
            into: "metadata.json".to_string(),
            content: GeneratedContent::MetadataDocument,
        }],
        fetch: vec![Fetched {
            into: "Screenshots".to_string(),
            source: FetchSource::Screenshots,
            name: "image_$index.$ext".to_string(),
        }],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveAction {
    pub pattern: String,
    pub target: String,
}
