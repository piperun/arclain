//! Tests for TabPluginPool. Loaded via `#[path]` so `super::*` is the
//! contents of plugin_instances.rs.

use super::*;
use anyhow::anyhow;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
struct MockInstance {
    id: u32,
}

#[test]
fn try_get_or_spawn_lazy_creates_on_first_call() {
    let pool: TabPluginPool<MockInstance> = TabPluginPool::default();
    let spawn_count = Arc::new(AtomicUsize::new(0));

    let counter = spawn_count.clone();
    let arc = pool
        .try_get_or_spawn("plugin_a", || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(MockInstance { id: 42 })
        })
        .unwrap();

    assert_eq!(arc.lock().id, 42);
    assert_eq!(spawn_count.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 1);
}

#[test]
fn try_get_or_spawn_returns_cached_on_second_call() {
    let pool: TabPluginPool<MockInstance> = TabPluginPool::default();
    let spawn_count = Arc::new(AtomicUsize::new(0));

    for _ in 0..3 {
        let counter = spawn_count.clone();
        pool.try_get_or_spawn("plugin_a", || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(MockInstance { id: 99 })
        })
        .unwrap();
    }

    // Spawned only once despite 3 calls.
    assert_eq!(spawn_count.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 1);
}

#[test]
fn different_plugin_ids_spawn_independently() {
    let pool: TabPluginPool<MockInstance> = TabPluginPool::default();

    pool.try_get_or_spawn("a", || Ok(MockInstance { id: 1 }))
        .unwrap();
    pool.try_get_or_spawn("b", || Ok(MockInstance { id: 2 }))
        .unwrap();
    pool.try_get_or_spawn("c", || Ok(MockInstance { id: 3 }))
        .unwrap();

    assert_eq!(pool.len(), 3);
}

#[test]
fn pool_drop_releases_all_instances() {
    struct DropTracker(Arc<AtomicUsize>);
    impl Drop for DropTracker {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let drop_count = Arc::new(AtomicUsize::new(0));
    {
        let pool: TabPluginPool<DropTracker> = TabPluginPool::default();
        let dc = drop_count.clone();
        pool.try_get_or_spawn("a", || Ok(DropTracker(dc.clone())))
            .unwrap();
        let dc = drop_count.clone();
        pool.try_get_or_spawn("b", || Ok(DropTracker(dc.clone())))
            .unwrap();
    } // pool dropped here

    assert_eq!(drop_count.load(Ordering::SeqCst), 2);
}

#[test]
fn spawn_error_leaves_slot_empty_for_retry() {
    let pool: TabPluginPool<MockInstance> = TabPluginPool::default();

    let result = pool.try_get_or_spawn("a", || Err(anyhow!("spawn failed")));
    assert!(result.is_err());
    assert_eq!(pool.len(), 0, "failed spawn must not occupy the slot");

    // Retry succeeds.
    let result2 = pool.try_get_or_spawn("a", || Ok(MockInstance { id: 7 }));
    assert!(result2.is_ok());
    assert_eq!(pool.len(), 1);
    assert_eq!(result2.unwrap().lock().id, 7);
}

#[test]
fn drop_instance_removes_one_keeps_others() {
    let pool: TabPluginPool<MockInstance> = TabPluginPool::default();
    pool.try_get_or_spawn("a", || Ok(MockInstance { id: 1 }))
        .unwrap();
    pool.try_get_or_spawn("b", || Ok(MockInstance { id: 2 }))
        .unwrap();
    assert_eq!(pool.len(), 2);

    pool.drop_instance("a");
    assert_eq!(pool.len(), 1);

    // Re-spawning "a" works.
    pool.try_get_or_spawn("a", || Ok(MockInstance { id: 99 }))
        .unwrap();
    assert_eq!(pool.len(), 2);
}

#[test]
fn drop_all_clears_everything() {
    let pool: TabPluginPool<MockInstance> = TabPluginPool::default();
    for id in ["a", "b", "c", "d"] {
        pool.try_get_or_spawn(id, || Ok(MockInstance { id: 0 }))
            .unwrap();
    }
    assert_eq!(pool.len(), 4);

    pool.drop_all();
    assert_eq!(pool.len(), 0);
}
