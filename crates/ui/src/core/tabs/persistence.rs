//! Serialize the tab list to disk on quit; restore on launch.
//!
//! Schema lives in `tabs.json` under the user config dir. Plugin
//! instances are NOT persisted — they re-spawn lazily on first use
//! after restore. Navigation breadcrumbs are also not persisted in
//! v1 (would require serializing the archive navigation history: the
//! current folder plus the back and forward stacks).

use super::tabs_collection::TabsCollection;
use super::TabId;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Per-tab data that survives across app restarts. Adding fields is a
/// schema change — bump `TabsSnapshot::version` and add a migration
/// branch in `restore_collection`.
///
/// New optional fields (defaulted via `#[serde(default)]`) are
/// backward-compatible without a version bump: older snapshots simply
/// see the field as missing and the default kicks in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabRestore {
    pub id: TabId,
    pub archive_path: Option<PathBuf>,
    /// Pinned state. Defaults to false so snapshots written before
    /// pinned tabs landed still load cleanly.
    #[serde(default)]
    pub pinned: bool,
}

/// On-disk snapshot of `TabsCollection`. The order of `tabs` is the
/// visible tab order in the bar; `active` references the TabId that
/// should be active on restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabsSnapshot {
    /// Schema version. v1 today. Bump and add a migration in
    /// `restore_collection` when fields change.
    pub version: u32,
    pub tabs: Vec<TabRestore>,
    pub active: TabId,
    /// Persisted so restored sessions keep generating new TabIds
    /// without colliding with restored ids.
    pub next_id: u64,
}

/// Capture the current `TabsCollection` as a snapshot.
pub fn snapshot(col: &TabsCollection) -> TabsSnapshot {
    TabsSnapshot {
        version: 1,
        tabs: col
            .tabs()
            .iter()
            .map(|t| TabRestore {
                id: t.id,
                archive_path: t.archive_path.get(),
                pinned: t.pinned.load(std::sync::atomic::Ordering::SeqCst),
            })
            .collect(),
        active: col.active_id(),
        next_id: col.peek_next_id(),
    }
}

/// Rebuild a `TabsCollection` from a snapshot. Calls
/// `TabsCollection::from_snapshot` which handles invariant edge
/// cases (e.g. snapshot with zero tabs → seed an empty placeholder).
pub fn restore_collection(snapshot: TabsSnapshot) -> TabsCollection {
    TabsCollection::from_snapshot(snapshot)
}

/// Serialize and write `tabs.json` to `path`. Creates parent
/// directories if needed.
pub fn save_collection(col: &TabsCollection, path: &Path) -> Result<()> {
    let snap = snapshot(col);
    let json = serde_json::to_string_pretty(&snap).context("serializing TabsSnapshot")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dir {:?}", parent))?;
    }
    std::fs::write(path, json).with_context(|| format!("writing {:?}", path))?;
    Ok(())
}

/// Read and deserialize `tabs.json` from `path`. Returns Err if the
/// file is missing, unreadable, or malformed. Callers fall back to
/// `TabsCollection::new()` (single empty tab) on Err.
pub fn load_collection(path: &Path) -> Result<TabsCollection> {
    let content = std::fs::read_to_string(path).with_context(|| format!("reading {:?}", path))?;
    let snap: TabsSnapshot = serde_json::from_str(&content).context("parsing tabs.json")?;
    Ok(restore_collection(snap))
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
