use super::*;
use crate::core::tabs::{TabId, TabState};
use std::sync::atomic::Ordering;

#[test]
fn op_guard_increments_and_decrements() {
    let tab = TabState::new(TabId(1));
    assert_eq!(tab.in_flight_ops.load(Ordering::SeqCst), 0);
    {
        let _guard = OpGuard::new(&tab);
        assert_eq!(tab.in_flight_ops.load(Ordering::SeqCst), 1);
    }
    assert_eq!(tab.in_flight_ops.load(Ordering::SeqCst), 0);
}

#[test]
fn nested_guards_count_up() {
    let tab = TabState::new(TabId(1));
    let g1 = OpGuard::new(&tab);
    let g2 = OpGuard::new(&tab);
    assert_eq!(tab.in_flight_ops.load(Ordering::SeqCst), 2);
    drop(g1);
    assert_eq!(tab.in_flight_ops.load(Ordering::SeqCst), 1);
    drop(g2);
    assert_eq!(tab.in_flight_ops.load(Ordering::SeqCst), 0);
}
