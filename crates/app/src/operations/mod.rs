//! The cancellable, event-broadcasting store every in-flight application
//! operation is tracked through.
//!
//! Everything here is crate-private: [`OperationRegistry`] is an
//! implementation detail behind the application facade a later task
//! adds, never a type a frontend names directly. A frontend only ever
//! sees the [`crate::ids::OperationId`], [`crate::event::OperationEvent`],
//! and [`crate::event::OperationSnapshot`] values its methods hand back.

pub(crate) mod registry;

// Not read anywhere yet: the facade (`ArclainApp`) is this re-export's only
// intended consumer, and it does not exist until a later task wires the
// registry into it.
#[allow(unused_imports)]
pub(crate) use registry::OperationRegistry;
