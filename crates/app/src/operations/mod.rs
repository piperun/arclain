//! The cancellable, event-broadcasting store every in-flight application
//! operation is tracked through, plus the public request type each
//! operation *kind* contributes.
//!
//! [`OperationRegistry`] and [`ChallengeWaiters`] are crate-private
//! implementation details behind the application facade: a frontend
//! never names them directly, only the [`crate::ids::OperationId`],
//! [`crate::event::OperationEvent`], and [`crate::event::OperationSnapshot`]
//! values their owning facade methods hand back. [`extract`] was the first
//! operation-kind submodule to also contribute genuinely public types --
//! [`extract::ExtractRequest`]/[`extract::CollisionPolicy`], the argument
//! `ArclainApp::start_extract` accepts -- and is re-exported here so a
//! caller can reach them as `arclain_app::operations::ExtractRequest` as
//! well as the fully-qualified path. `convert`/`organize`/`pipeline`
//! follow the same shape: one submodule per `start_*` processing
//! operation, each owning its own request type plus request-shaped
//! (no-I/O) validation, re-exported the same way.

pub mod archive_mutation;
pub(crate) mod challenge_waiters;
pub mod extract;
pub(crate) mod registry;

pub mod convert;
pub mod merge;
pub mod organize;
pub mod pipeline;

pub use archive_mutation::ArchiveMutationRequest;
pub(crate) use challenge_waiters::ChallengeWaiters;
pub use extract::{CollisionPolicy, ExtractRequest};
pub(crate) use registry::OperationRegistry;

pub use convert::ConvertRequest;
pub use merge::{MergeCompressionLevel, MergeOutputFormat, MergeRequest};
pub use organize::OrganizeRequest;
pub use pipeline::PipelineRequest;
