//! Core data types for archive processing pipelines.

use crate::features::conversion::{CompressionLevel, ConvertFormat};
use std::path::{Path, PathBuf};

/// A single operation that can appear in a pipeline.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PipelineStep {
    /// Unwrap inner archives as sibling folders.
    ///
    /// `max_depth` controls how many times the extractor re-scans for archives
    /// that appeared after a previous iteration unpacked an outer archive into
    /// a subtree. A pathological input like `.rar → folder → .zip → folder → .7z`
    /// needs more than one pass. `1` = one pass (original behavior), `0` = run
    /// until the tree stabilizes (still bounded by an internal safety cap).
    Flatten {
        strip_common_prefix: bool,
        #[serde(default = "default_flatten_max_depth")]
        max_depth: u32,
    },
    /// Apply an organization rule by its database id.
    Organize { rule_id: i64 },
    /// Convert the final layout to a target format.
    Convert {
        format: ConvertFormat,
        compression: CompressionLevel,
        password: Option<String>,
    },
}

/// Preserve the historical single-pass behavior when deserializing old presets.
fn default_flatten_max_depth() -> u32 {
    1
}

impl PipelineStep {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Flatten { .. } => "Flatten nested archives",
            Self::Organize { .. } => "Apply organization rule",
            Self::Convert { .. } => "Convert format",
        }
    }

    /// Predict where this step will write, if anywhere, given the pipeline's
    /// input path and output mode. `None` means the step doesn't produce a
    /// user-visible artifact (Flatten mutates `work_dir` in place).
    ///
    /// Prediction is best-effort for `Organize`: without the extracted
    /// archive's entries + metadata we can't resolve a rule's
    /// `root_folder_template`. Preview code should accept `None` for Organize
    /// and rely on the executor's at-runtime collision check instead.
    pub fn predicted_output_static(
        &self,
        input: &Path,
        output_mode: &PipelineOutput,
    ) -> Option<OutputIdentity> {
        match self {
            Self::Flatten { .. } => None,
            Self::Organize { .. } => None, // needs runtime context
            Self::Convert { format, .. } => Some(OutputIdentity {
                kind: OutputKind::Archive,
                path: output_mode.resolve(input, format.extension()),
            }),
        }
    }
}

/// What the pipeline operates on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PipelineInput {
    /// One or more explicit archive files.
    Files(Vec<PathBuf>),
    /// All archives inside a folder (non-recursive).
    Folder(PathBuf),
}

impl PipelineInput {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Files(v) => v.is_empty(),
            Self::Folder(p) => !p.exists(),
        }
    }
}

/// Where pipeline outputs land.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PipelineOutput {
    /// Output next to each input (e.g. input.rar → input.zip).
    SameFolder,
    /// Output to a specific folder.
    NewFolder(PathBuf),
}

impl Default for PipelineOutput {
    fn default() -> Self {
        Self::SameFolder
    }
}

impl PipelineOutput {
    /// Resolve the output path for a given input file with the target extension.
    pub fn resolve(&self, input: &Path, ext: &str) -> PathBuf {
        let stem = input.file_stem().unwrap_or_default();
        match self {
            Self::SameFolder => {
                let mut p = input.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                let mut name = stem.to_os_string();
                name.push(format!(".{}", ext));
                p.push(name);
                p
            }
            Self::NewFolder(folder) => {
                let mut p = folder.clone();
                let mut name = stem.to_os_string();
                name.push(format!(".{}", ext));
                p.push(name);
                p
            }
        }
    }
}

/// Complete pipeline specification.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pipeline {
    pub input: Option<PipelineInput>,
    pub steps: Vec<PipelineStep>,
    pub output: PipelineOutput,
    /// Per-pipeline override for output-collision handling.
    /// `None` = inherit the app's `default_collision_policy` setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collision_policy: Option<OutputCollisionPolicy>,
}

impl Pipeline {
    /// Resolve the effective collision policy for this pipeline: returns the
    /// per-pipeline override if set, otherwise the caller's default.
    pub fn effective_collision_policy(
        &self,
        default: OutputCollisionPolicy,
    ) -> OutputCollisionPolicy {
        self.collision_policy.unwrap_or(default)
    }

    /// Blake3 hash of the pipeline's configuration (hex-encoded).
    ///
    /// Excludes `input` so two runs of the same pipeline against different
    /// archives share the same `pipeline_hash`. Together with the input's
    /// content hash it forms the dedup key for `pipeline_runs`.
    ///
    /// Stability: relies on serde_json's default struct-field ordering, which
    /// is source order. All the hashed types are structs/enums (no HashMaps),
    /// so the byte output is deterministic across runs as long as the type
    /// definitions don't change. If a type gains a new field, hashes drift
    /// and every previous run re-runs — which is the correct behavior.
    pub fn config_hash(&self) -> String {
        #[derive(serde::Serialize)]
        struct Hashable<'a> {
            steps: &'a [PipelineStep],
            output: &'a PipelineOutput,
            collision_policy: &'a Option<OutputCollisionPolicy>,
        }
        let hashable = Hashable {
            steps: &self.steps,
            output: &self.output,
            collision_policy: &self.collision_policy,
        };
        let bytes = serde_json::to_vec(&hashable).unwrap_or_else(|_| Vec::new());
        blake3::hash(&bytes).to_hex().to_string()
    }
}

/// What happens when a producing step is about to write to a path that
/// already exists on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OutputCollisionPolicy {
    /// Fail the step if the path exists.
    Fail,
    /// Silently skip the step; leave existing content untouched.
    Skip,
    /// Unconditionally overwrite.
    Overwrite,
    /// Phase 3: consult `pipeline_runs` + hash. Until Phase 3 lands, `Smart`
    /// degrades to `Fail` so users never get silent unexpected behavior.
    Smart,
}

impl Default for OutputCollisionPolicy {
    fn default() -> Self {
        Self::Smart
    }
}

impl OutputCollisionPolicy {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Fail => "Fail on existing",
            Self::Skip => "Skip if exists",
            Self::Overwrite => "Overwrite",
            Self::Smart => "Smart (dedup / prompt)",
        }
    }

    /// Serialize to the stable identifier used in settings storage (app_config).
    pub fn to_settings_str(&self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Skip => "skip",
            Self::Overwrite => "overwrite",
            Self::Smart => "smart",
        }
    }

    /// Parse the settings identifier back into a policy. Unknown values return None.
    pub fn from_settings_str(s: &str) -> Option<Self> {
        match s {
            "fail" => Some(Self::Fail),
            "skip" => Some(Self::Skip),
            "overwrite" => Some(Self::Overwrite),
            "smart" => Some(Self::Smart),
            _ => None,
        }
    }
}

/// Key under which the app-wide default collision policy is stored in
/// `app_config`. Effective policy is: pipeline override (if set) else this
/// setting else hardcoded `Smart`.
pub const COLLISION_POLICY_CONFIG_KEY: &str = "pipeline.default_collision_policy";

/// What a producing step is about to write. Computed just before the step
/// runs so the collision gate can consult the filesystem (and in Phase 3,
/// the pipeline_runs DB).
#[derive(Debug, Clone)]
pub struct OutputIdentity {
    pub kind: OutputKind,
    pub path: PathBuf,
}

/// What artifact a step produces. Drives collision-check behavior (a missing
/// file isn't the same as a missing folder).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    /// A single file (e.g. Convert → archive.zip).
    Archive,
    /// A directory (e.g. Organize → MyGame/).
    Folder,
}

/// Preset for opening the Process page pre-configured.
/// Used by the toolbar shortcuts (Convert..., Organize, etc.).
#[derive(Debug, Clone)]
pub enum ProcessPreset {
    /// Opens with Convert step added.
    ConvertSingleFile(PathBuf),
    /// Opens with folder input populated, no steps.
    BatchFolder(PathBuf),
    /// Opens with Organize step for the current archive.
    OrganizeSingleFile(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_display_names() {
        assert_eq!(
            PipelineStep::Flatten {
                strip_common_prefix: false,
                max_depth: 1,
            }
            .display_name(),
            "Flatten nested archives"
        );
    }

    #[test]
    fn flatten_deserializes_without_max_depth() {
        // Old presets written before max_depth existed should still load
        // with the historical single-pass default.
        let json = r#"{"Flatten":{"strip_common_prefix":true}}"#;
        let step: PipelineStep = serde_json::from_str(json).unwrap();
        assert_eq!(
            step,
            PipelineStep::Flatten {
                strip_common_prefix: true,
                max_depth: 1,
            }
        );
    }

    #[test]
    fn flatten_roundtrips_with_max_depth() {
        let original = PipelineStep::Flatten {
            strip_common_prefix: false,
            max_depth: 5,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: PipelineStep = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn default_pipeline_is_empty() {
        let p = Pipeline::default();
        assert!(p.input.is_none());
        assert!(p.steps.is_empty());
        assert_eq!(p.output, PipelineOutput::SameFolder);
    }

    #[test]
    fn input_is_empty_variants() {
        assert!(PipelineInput::Files(vec![]).is_empty());
        assert!(!PipelineInput::Files(vec![PathBuf::from("a.rar")]).is_empty());
    }

    #[test]
    fn output_resolve_same_folder() {
        let input = PathBuf::from("/src/mod.rar");
        let output = PipelineOutput::SameFolder;
        assert_eq!(output.resolve(&input, "zip"), PathBuf::from("/src/mod.zip"));
    }

    #[test]
    fn output_resolve_new_folder() {
        let input = PathBuf::from("/src/mod.rar");
        let output = PipelineOutput::NewFolder(PathBuf::from("/dst"));
        assert_eq!(output.resolve(&input, "7z"), PathBuf::from("/dst/mod.7z"));
    }
}
