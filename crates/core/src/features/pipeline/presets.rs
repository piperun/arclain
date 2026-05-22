//! Pipeline preset (de)serialization — persists to user config as JSON.

use super::types::Pipeline;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPreset {
    pub name: String,
    pub pipeline: Pipeline,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PresetsFile {
    pub presets: Vec<SavedPreset>,
}

/// Default presets shipped with the app.
pub fn builtin_presets() -> Vec<SavedPreset> {
    use super::types::{PipelineOutput, PipelineStep};
    use crate::features::conversion::{CompressionLevel, ConvertFormat};

    vec![
        SavedPreset {
            name: "RE Mod Cleanup".to_string(),
            pipeline: Pipeline {
                input: None,
                steps: vec![
                    PipelineStep::Flatten {
                        strip_common_prefix: true,
                        max_depth: 0, // recursive — mod packs often have nested .rar → .zip wrappers
                    },
                    PipelineStep::Convert {
                        format: ConvertFormat::Zip,
                        compression: CompressionLevel::Normal,
                        password: None,
                    },
                ],
                output: PipelineOutput::SameFolder,
                collision_policy: None,
                output_artifact: Default::default(),
            },
        },
        SavedPreset {
            name: "Convert to 7z (Max)".to_string(),
            pipeline: Pipeline {
                input: None,
                steps: vec![PipelineStep::Convert {
                    format: ConvertFormat::SevenZ,
                    compression: CompressionLevel::Max,
                    password: None,
                }],
                output: PipelineOutput::SameFolder,
                collision_policy: None,
                output_artifact: Default::default(),
            },
        },
    ]
}

/// Load presets from a file. Returns builtins on missing/corrupt file.
pub fn load_presets(path: &Path) -> Vec<SavedPreset> {
    if !path.exists() {
        return builtin_presets();
    }
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("[presets] read failed: {} — using builtins", e);
            return builtin_presets();
        }
    };
    match serde_json::from_str::<PresetsFile>(&raw) {
        Ok(pf) => pf.presets,
        Err(e) => {
            tracing::warn!("[presets] parse failed: {} — using builtins", e);
            builtin_presets()
        }
    }
}

/// Save presets to a file (atomic: write temp + rename).
pub fn save_presets(path: &Path, presets: &[SavedPreset]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let raw = serde_json::to_string_pretty(&PresetsFile {
        presets: presets.to_vec(),
    })
    .context("serialize presets")?;
    fs::write(&tmp, raw).context("write tmp presets")?;
    fs::rename(&tmp, path).context("atomic rename presets")?;
    Ok(())
}

/// Resolve the default presets path (user config dir).
pub fn default_presets_path() -> Option<PathBuf> {
    let dir = arclain_app_fs::AppDirectories::init("arclain", None).ok()?;
    Some(dir.config_dir.join("pipeline_presets.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_presets() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("presets.json");

        let original = builtin_presets();
        save_presets(&path, &original).unwrap();
        let loaded = load_presets(&path);

        assert_eq!(loaded.len(), original.len());
        assert_eq!(loaded[0].name, "RE Mod Cleanup");
    }

    #[test]
    fn missing_file_returns_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nothing.json");
        let loaded = load_presets(&path);
        assert!(!loaded.is_empty());
    }

    #[test]
    fn corrupt_file_returns_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.json");
        fs::write(&path, "not json").unwrap();
        let loaded = load_presets(&path);
        assert!(!loaded.is_empty());
    }
}
