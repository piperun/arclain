//! Opaque, process-local identifiers used across the application facade.
//!
//! Every identifier below is a transparent wrapper around a `u64`. Values
//! are minted by whichever store owns the corresponding resource (archive
//! sessions, challenges, entries, operations, plugin sessions,
//! materialization leases); nothing in this crate hands out identifiers on
//! its own except [`CorrelationId::generate`], which is a plain
//! process-wide counter, not a store.
//!
//! `from_raw`/`into_raw` exist so a caller that only ever passes an
//! identifier through opaque channels -- CLI arguments, bridge payloads,
//! persisted UI state -- can round-trip the value without this crate
//! exposing its internal representation as a public field. Reconstructing
//! an identifier from a raw value does not by itself prove the identifier
//! is valid: every facade method that accepts one validates it against the
//! store that owns it.

use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            serde::Deserialize,
            serde::Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            pub const fn from_raw(value: u64) -> Self {
                Self(value)
            }

            pub const fn into_raw(self) -> u64 {
                self.0
            }
        }
    };
}

opaque_id!(ArchiveSessionId);
opaque_id!(ChallengeId);
opaque_id!(CorrelationId);
opaque_id!(EntryId);
opaque_id!(MaterializationLeaseId);
opaque_id!(OperationId);
opaque_id!(PluginSessionId);

impl CorrelationId {
    /// Generates a new correlation ID, unique for the lifetime of this
    /// process. Backed by a monotonic atomic counter rather than a UUID or
    /// random source: correlation IDs only need to disambiguate errors
    /// within one running process (logs, bridge event streams), never
    /// across process restarts or machines, so a network service or a
    /// `rand`/`uuid` dependency would be pure overhead here.
    pub fn generate() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}
