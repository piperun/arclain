//! Archive processing pipelines — compose operations into ordered workflows.

pub mod context;
pub mod executor;
pub mod preview;
pub mod types;

pub use context::PipelineContext;
pub use executor::{execute_pipeline, PipelineProgress};
pub use preview::{preview_pipeline, PipelinePreview, PreviewEntry};
pub use types::{Pipeline, PipelineInput, PipelineOutput, PipelineStep, ProcessPreset};
