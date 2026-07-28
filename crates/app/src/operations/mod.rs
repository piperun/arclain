//! The cancellable, event-broadcasting store every in-flight application
//! operation is tracked through, plus the public request type each
//! operation *kind* contributes.
//!
//! [`OperationRegistry`] and [`ChallengeWaiters`] are crate-private
//! implementation details behind the application facade: a frontend
//! never names them directly, only the [`crate::ids::OperationId`],
//! [`crate::event::OperationEvent`], and [`crate::event::OperationSnapshot`]
//! values their owning facade methods hand back. [`extract`] is the first
//! operation-kind submodule to also contribute genuinely public types --
//! [`extract::ExtractRequest`]/[`extract::CollisionPolicy`], the argument
//! `ArclainApp::start_extract` accepts -- and is re-exported here so a
//! caller can reach them as `arclain_app::operations::ExtractRequest` as
//! well as the fully-qualified path. Later tasks adding `start_convert`/
//! `start_organize`/etc. are expected to add their own submodule
//! following the same shape.

pub(crate) mod challenge_waiters;
pub mod extract;
pub(crate) mod registry;

pub(crate) use challenge_waiters::ChallengeWaiters;
pub use extract::{CollisionPolicy, ExtractRequest};
pub(crate) use registry::OperationRegistry;
