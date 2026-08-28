//! Path-tree helper for `prune_entries` and `find_game_content_root`.
//!
//! `RuleEngine::prune_entries` builds a `TreeNode` from the archive's
//! flat entry list, prunes 0-byte files and empty directories, then
//! re-flattens. Lifted out of `engine.rs` so the plan-builder file
//! doesn't carry tree-walk plumbing alongside the planning logic.

use crate::ArchiveEntry;
use std::collections::HashMap;

pub(super) struct TreeNode {
    pub(super) entry: Option<ArchiveEntry>,
    pub(super) children: HashMap<String, TreeNode>,
    pub(super) is_dir: bool,
}

impl TreeNode {
    pub(super) fn new(is_dir: bool) -> Self {
        Self {
            entry: None,
            children: HashMap::new(),
            is_dir,
        }
    }

    pub(super) fn insert(&mut self, path: &str, entry: ArchiveEntry) {
        let parts: Vec<&str> = path
            .split(|c| c == '/' || c == '\\')
            .filter(|s| !s.is_empty())
            .collect();
        self.insert_recursive(&parts, entry);
    }

    fn insert_recursive(&mut self, parts: &[&str], entry: ArchiveEntry) {
        if parts.is_empty() {
            // This node represents the entry itself
            self.is_dir = entry.is_dir;
            self.entry = Some(entry);
            return;
        }

        let name = parts[0];
        let child = self
            .children
            .entry(name.to_string())
            .or_insert_with(|| TreeNode::new(true));
        child.insert_recursive(&parts[1..], entry);
    }

    pub(super) fn prune(&mut self) -> bool {
        // Returns true if this node should be kept, false if it should be removed

        // 1. Prune children first (bottom-up)
        let mut to_remove = Vec::new();
        for (name, child) in &mut self.children {
            if !child.prune() {
                to_remove.push(name.clone());
            }
        }

        for name in to_remove {
            self.children.remove(&name);
        }

        // 2. Check if this node is unnecessary

        // If it's a file
        if !self.is_dir {
            if let Some(entry) = &self.entry {
                if entry.size == 0 {
                    return false; // Remove 0-byte file
                }
            }
            return true; // Keep non-zero file
        }

        // If it's a directory
        if self.children.is_empty() {
            // Empty directory -> Remove
            return false;
        }

        true // Keep directory with children
    }

    pub(super) fn flatten(&self) -> Vec<ArchiveEntry> {
        let mut result = Vec::new();

        // Only include files, not directories
        if let Some(entry) = &self.entry {
            if !entry.is_dir {
                result.push(entry.clone());
            }
        }

        // Sort children by name to ensure deterministic flattening order.
        // HashMap iteration is not stable between runs, so flattening
        // would return entries in different order each time.
        let mut sorted_names: Vec<_> = self.children.keys().collect();
        sorted_names.sort();
        for name in sorted_names {
            if let Some(child) = self.children.get(name) {
                result.extend(child.flatten());
            }
        }
        result
    }
}
