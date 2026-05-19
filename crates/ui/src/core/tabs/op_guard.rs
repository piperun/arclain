//! RAII guard for tracking per-tab in-flight operations.
//!
//! Constructing an OpGuard increments the tab's `in_flight_ops`
//! counter; dropping it decrements. Owners hold a guard for the
//! lifetime of a background operation so `TabsCollection::close`
//! can correctly distinguish "tab has running ops, blocks close"
//! from "tab is idle, close immediately".

use super::TabState;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub struct OpGuard {
    counter: Arc<AtomicUsize>,
}

impl OpGuard {
    pub fn new(tab: &TabState) -> Self {
        tab.in_flight_ops.fetch_add(1, Ordering::SeqCst);
        Self {
            counter: tab.in_flight_ops.clone(),
        }
    }
}

impl Drop for OpGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
#[path = "op_guard_tests.rs"]
mod tests;
