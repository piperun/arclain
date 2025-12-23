//! Effect - side effects that run when dependencies change
//!
//! An `Effect` runs a callback when manually triggered or when subscribed signals change.

use std::sync::Arc;

/// A side effect that can be triggered manually or via signal subscription.
///
/// # Example
/// ```
/// use arclain_signals::{Signal, Effect};
///
/// let count = Signal::new(0);
/// let effect = Effect::new(|| println!("Effect ran!"));
///
/// // Run effect manually
/// effect.run();
///
/// // Or subscribe to a signal
/// count.subscribe(effect.callback());
/// count.set(1); // Effect runs
/// ```
pub struct Effect {
    action: Arc<dyn Fn() + Send + Sync>,
}

impl Effect {
    /// Create a new effect with the given action.
    pub fn new<F>(action: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self {
            action: Arc::new(action),
        }
    }

    /// Run the effect manually.
    pub fn run(&self) {
        (self.action)();
    }

    /// Get a callback that can be used to subscribe to a signal.
    pub fn callback(&self) -> impl Fn() + Send + Sync + 'static {
        let action = Arc::clone(&self.action);
        move || action()
    }
}

impl Clone for Effect {
    fn clone(&self) -> Self {
        Self {
            action: Arc::clone(&self.action),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_effect_run() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let effect = Effect::new(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        effect.run();
        effect.run();

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_effect_with_signal() {
        use crate::Signal;

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let effect = Effect::new(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        let signal = Signal::new(0);
        signal.subscribe(effect.callback());

        signal.set(1);
        signal.set(2);

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
