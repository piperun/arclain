//! Computed values - derived reactive state
//!
//! A `Computed<T>` derives its value from other signals and auto-updates.

use parking_lot::RwLock;
use std::sync::Arc;

/// A computed value that derives from other reactive values.
///
/// # Example
/// ```
/// use arclain_signals::{Signal, Computed};
///
/// let count = Signal::new(5);
/// let count_clone = count.clone();
/// let doubled = Computed::new(move || count_clone.get() * 2);
///
/// assert_eq!(doubled.get(), 10);
/// count.set(10);
/// assert_eq!(doubled.get(), 20);
/// ```
pub struct Computed<T> {
    compute: Arc<dyn Fn() -> T + Send + Sync>,
    cached: RwLock<Option<T>>,
}

impl<T: Clone> Computed<T> {
    /// Create a new computed value with the given computation function.
    pub fn new<F>(compute: F) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        Self {
            compute: Arc::new(compute),
            cached: RwLock::new(None),
        }
    }

    /// Get the computed value.
    /// Currently recomputes on every call. Future optimization: cache invalidation.
    pub fn get(&self) -> T {
        (self.compute)()
    }

    /// Force recomputation and return the new value.
    pub fn recompute(&self) -> T {
        let value = (self.compute)();
        *self.cached.write() = Some(value.clone());
        value
    }
}

impl<T: Clone> Clone for Computed<T> {
    fn clone(&self) -> Self {
        Self {
            compute: Arc::clone(&self.compute),
            cached: RwLock::new(self.cached.read().clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Signal;

    #[test]
    fn test_computed_basic() {
        let signal = Signal::new(5);
        let signal_clone = signal.clone();
        let computed = Computed::new(move || signal_clone.get() * 2);

        assert_eq!(computed.get(), 10);
    }

    #[test]
    fn test_computed_updates() {
        let signal = Signal::new(3);
        let signal_clone = signal.clone();
        let computed = Computed::new(move || signal_clone.get() + 1);

        assert_eq!(computed.get(), 4);
        signal.set(10);
        assert_eq!(computed.get(), 11);
    }
}
