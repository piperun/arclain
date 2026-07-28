//! The cancellable, event-broadcasting store every in-flight application
//! operation is tracked through.
//!
//! Everything here is crate-private: [`OperationRegistry`] is an
//! implementation detail behind the application facade a later task
//! adds, never a type a frontend names directly. A frontend only ever
//! sees the [`crate::ids::OperationId`], [`crate::event::OperationEvent`],
//! and [`crate::event::OperationSnapshot`] values its methods hand back.

pub(crate) mod challenge_waiters;
pub(crate) mod registry;

pub(crate) use challenge_waiters::ChallengeWaiters;
pub(crate) use registry::OperationRegistry;
