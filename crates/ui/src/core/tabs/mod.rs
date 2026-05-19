//! Multi-archive tab support.
//!
//! Each `TabState` owns the per-tab archive-context signals
//! (archive_path, entries, navigation, etc.) that previously lived on
//! the global `AppSignals`. `TabsCollection` holds the ordered list
//! plus the currently-active TabId.
//!
//! Background operations should capture `Arc<TabState>` at spawn time
//! (not resolve `tabs.active()` lazily at completion) so results land
//! in the originating tab even after the user switches.

pub mod op_guard;
pub mod persistence;
pub mod plugin_instances;
pub mod tab_state;
pub mod tabs_collection;

pub use op_guard::OpGuard;
pub use persistence::{load_collection, save_collection, snapshot, TabRestore, TabsSnapshot};
pub use plugin_instances::TabPluginPool;
pub use tab_state::TabState;
pub use tabs_collection::{CloseResult, TabsCollection};

use serde::{Deserialize, Serialize};

/// Monotonic per-app-session identifier for a tab.
///
/// Never reused within a session. Persisted across restart so restored
/// sessions resume their counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TabId(pub u64);
