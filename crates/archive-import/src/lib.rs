//! Archive-only import: Arclain decodes; the caller owns every destination.
mod client;
mod protocol;
#[cfg(feature = "worker")]
pub mod worker;
pub use client::{ArchiveClient, ArchiveSession, Cancellation, WorkerOptions};
pub use protocol::ImportError;
pub use unboxer::{EntryName, Inventory, ObjectKind, PackedBlock, PhysicalEntry};
