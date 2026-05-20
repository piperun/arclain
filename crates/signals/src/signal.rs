//! Reactive Signal type - the core building block
//!
//! A `Signal<T>` holds a value and notifies subscribers when it changes.

use parking_lot::RwLock;
use std::sync::Arc;

type Listener = Arc<dyn Fn() + Send + Sync>;

/// A reactive signal that holds a value and notifies listeners on change.
///
/// # Example
/// ```
/// use arclain_signals::Signal;
///
/// let count = Signal::new(0);
///
/// // Subscribe to changes
/// count.subscribe(|| println!("Value changed!"));
///
/// // Update value (triggers notification)
/// count.set(42);
///
/// // Read value
/// assert_eq!(count.get(), 42);
/// ```
pub struct Signal<T> {
    inner: Arc<SignalInner<T>>,
}

struct SignalInner<T> {
    value: RwLock<T>,
    listeners: RwLock<Vec<Listener>>,
    name: Option<String>,
    tracing: bool,
    location: &'static std::panic::Location<'static>,
}

impl<T> Signal<T> {
    /// Create a new signal with the given initial value.
    #[track_caller]
    pub fn new(initial: T) -> Self {
        Self {
            inner: Arc::new(SignalInner {
                value: RwLock::new(initial),
                listeners: RwLock::new(Vec::new()),
                name: None,
                tracing: false,
                location: std::panic::Location::caller(),
            }),
        }
    }

    /// Builder: Set a debug name for this signal
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.name = Some(name.into());
        } else {
            panic!("Cannot set name on shared Signal. Call .with_name() before cloning.");
        }
        self
    }

    /// Builder: Enable debug tracing for this signal
    pub fn with_tracing(mut self) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.tracing = true;
        } else {
            panic!("Cannot set tracing on shared Signal. Call .with_tracing() before cloning.");
        }
        self
    }

    /// Subscribe to value changes. The callback is invoked whenever `set()` is called.
    pub fn subscribe<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.inner.listeners.write().push(Arc::new(callback));
    }

    /// Notify all listeners that the value has changed.
    ///
    /// Listeners are snapshotted into a `Vec<Arc<_>>` under a brief read guard;
    /// the guard is released before any callback is invoked. This avoids
    /// self-deadlock if a callback re-enters via `subscribe` (which takes the
    /// write lock on the same RwLock).
    fn notify(&self) {
        let listeners: Vec<Listener> = {
            let guard = self.inner.listeners.read();
            let count = guard.len();

            // Tracing logic: Only trace if explicitly enabled for this signal
            if self.inner.tracing && count > 0 {
                let name = self.inner.name.as_deref().unwrap_or("unnamed");
                let loc = self.inner.location;

                tracing::trace!(
                    "[SIGNAL] notify [{}] at {}:{} - listeners: {}",
                    name,
                    loc.file(),
                    loc.line(),
                    count
                );
            }

            guard.iter().cloned().collect()
        };

        for listener in &listeners {
            listener();
        }
    }

    /// Get a read guard to the value.
    pub fn read(&self) -> parking_lot::RwLockReadGuard<'_, T> {
        self.inner.value.read()
    }

    /// Get a write guard to the value.
    /// When the guard is dropped, listeners are notified.
    pub fn write(&self) -> SignalWriteGuard<'_, T> {
        SignalWriteGuard {
            guard: self.inner.value.write(),
            signal_inner: &self.inner,
        }
    }
}

/// A write guard that triggers notification when dropped.
pub struct SignalWriteGuard<'a, T> {
    guard: parking_lot::RwLockWriteGuard<'a, T>,
    signal_inner: &'a SignalInner<T>,
}

impl<'a, T> std::ops::Deref for SignalWriteGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &*self.guard
    }
}

impl<'a, T> std::ops::DerefMut for SignalWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.guard
    }
}

impl<'a, T> Drop for SignalWriteGuard<'a, T> {
    fn drop(&mut self) {
        // Notify listeners on drop.
        // Snapshot the listener Vec under a brief read guard, then release
        // before invoking callbacks (same self-deadlock avoidance as Signal::notify).
        let listeners: Vec<Listener> = {
            let guard = self.signal_inner.listeners.read();
            guard.iter().cloned().collect()
        };
        for listener in &listeners {
            listener();
        }
    }
}

impl<T: Clone> Signal<T> {
    /// Get the current value.
    pub fn get(&self) -> T {
        self.inner.value.read().clone()
    }

    /// Set a new value and notify all listeners.
    pub fn set(&self, value: T) {
        *self.inner.value.write() = value;
        self.notify();
    }

    /// Update the value using a function and notify listeners.
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
    {
        {
            let mut guard = self.inner.value.write();
            f(&mut *guard);
        }
        self.notify();
    }
}

impl<T: Clone + PartialEq> Signal<T> {
    /// Set a new value only if it differs from the current value.
    /// Returns true if the value was changed, false otherwise.
    /// This prevents unnecessary repaint cycles when the value hasn't changed.
    pub fn set_if_changed(&self, value: T) -> bool {
        let current = self.inner.value.read().clone();
        if current != value {
            drop(current);
            *self.inner.value.write() = value;
            self.notify();
            true
        } else {
            false
        }
    }
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: Default> Default for Signal<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_signal_get_set() {
        let signal = Signal::new(42);
        assert_eq!(signal.get(), 42);
        signal.set(100);
        assert_eq!(signal.get(), 100);
    }

    #[test]
    fn test_signal_subscribe() {
        let signal = Signal::new(0);
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        signal.subscribe(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        signal.set(1);
        signal.set(2);
        signal.set(3);

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_signal_update() {
        let signal = Signal::new(vec![1, 2, 3]);
        signal.update(|v| v.push(4));
        assert_eq!(signal.get(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_signal_write_guard() {
        let signal = Signal::new(0);
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        signal.subscribe(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        {
            let mut guard = signal.write();
            *guard = 5;
        } // Drop happens here, should notify

        assert_eq!(signal.get(), 5);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_signal_clone() {
        let signal1 = Signal::new(10);
        let signal2 = signal1.clone();

        signal1.set(20);
        assert_eq!(signal2.get(), 20);
    }

    /// Regression test for R7: a callback invoked during notify must be able
    /// to call `subscribe` on the same signal without self-deadlocking.
    ///
    /// Before R7, both `Signal::notify` and `SignalWriteGuard::drop` held the
    /// listeners read guard while invoking callbacks. A callback that called
    /// `subscribe` (write on same RwLock) would deadlock the thread.
    ///
    /// The snapshot-then-iterate fix releases the read guard before any
    /// callback runs, so re-entry is safe.
    #[test]
    fn test_subscribe_during_notify_does_not_deadlock() {
        let signal: Signal<i32> = Signal::new(0);
        let added = Arc::new(AtomicUsize::new(0));

        // First callback subscribes a second one when fired.
        let signal_inner = signal.clone();
        let added_inner = added.clone();
        signal.subscribe(move || {
            // This write previously deadlocked under the held read guard.
            let added_for_new = added_inner.clone();
            signal_inner.subscribe(move || {
                added_for_new.fetch_add(1, Ordering::SeqCst);
            });
        });

        // Trigger notify. Without the R7 fix this hangs forever.
        signal.set(1);

        // The newly-added callback should not have fired for THIS notify
        // (it was registered after the snapshot was taken), but should fire next.
        assert_eq!(added.load(Ordering::SeqCst), 0);

        signal.set(2);
        // Now the subscribed-during-callback listener should have fired.
        // Note: the outer callback also fires again on each set, adding more
        // listeners — we just need to confirm at least one new listener ran.
        assert!(added.load(Ordering::SeqCst) >= 1);
    }
}
