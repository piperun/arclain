//! Archive processing pipelines — compose operations into ordered workflows.

pub mod apply_plan;
pub mod context;
pub mod executor;
pub mod hashing;
mod output_transaction;
pub mod presets;
pub mod preview;
pub mod types;

pub use context::PipelineContext;
pub use executor::{execute_pipeline, PipelineProgress};
pub use presets::{
    builtin_presets, default_presets_path, load_presets, save_presets, PresetsFile, SavedPreset,
};
pub use preview::{
    preview_pipeline, preview_pipeline_with_metadata, PipelinePreview, PreviewEntry,
};
pub use types::{
    OutputArtifact, OutputCollisionPolicy, OutputIdentity, OutputKind, Pipeline, PipelineInput,
    PipelineOutput, PipelineStep, ProcessPreset, COLLISION_POLICY_CONFIG_KEY,
};
