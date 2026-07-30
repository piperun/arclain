//! A cache slot whose load intent auto-fires at most once per arming.
//!
//! The MVU pages auto-fire their initial data load from the render
//! pass: "if the cache is empty, emit `Load...`". Left bare, quenching
//! that intent depends on the dispatcher *succeeding* — a load that
//! returns `Err` leaves the cache empty, so the page re-fires a
//! blocking database call every frame, forever.
//!
//! [`LoadSlot`] makes the quench structural instead: the render pass
//! consumes the one armed shot via [`LoadSlot::try_fire`] whether or
//! not the dispatcher later succeeds, and only an explicit user action
//! arms the next one — a Retry click ([`LoadSlot::rearm`]) or a cache
//! invalidation after a mutation ([`LoadSlot::invalidate`]). A failed
//! load therefore fires exactly once per user action, and the success
//! path is unchanged: the slot starts armed, so the first render still
//! emits its load.

/// `Option<T>`-shaped cache plus the arming state of the auto-fired
/// intent that fills it. See the module doc for why the two travel
/// together: every transition back to "empty" re-arms in the same
/// call, so no code path can empty the cache and strand the page
/// without a next shot.
#[derive(Debug)]
pub struct LoadSlot<T> {
    data: Option<T>,
    /// Whether [`try_fire`](Self::try_fire) may return `true`. Consumed
    /// by the render pass, restored only by [`set`](Self::set),
    /// [`invalidate`](Self::invalidate) and [`rearm`](Self::rearm).
    armed: bool,
}

impl<T> Default for LoadSlot<T> {
    /// Empty and armed: the first render fires the load.
    fn default() -> Self {
        Self {
            data: None,
            armed: true,
        }
    }
}

impl<T> LoadSlot<T> {
    /// The cached value, `None` while unloaded (or invalidated).
    pub fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }

    /// Store a loaded value (the dispatcher's success path) and re-arm
    /// for whenever the cache next empties.
    pub fn set(&mut self, value: T) {
        self.data = Some(value);
        self.armed = true;
    }

    /// Drop the cached value and arm one reload. This is the "refresh
    /// after a mutation" transition, and it is deliberately the only
    /// way to empty the slot.
    pub fn invalidate(&mut self) {
        self.data = None;
        self.armed = true;
    }

    /// Arm one more shot without touching the (empty) cache — the
    /// Retry affordance of a held failure.
    pub fn rearm(&mut self) {
        self.armed = true;
    }

    /// `true` exactly once per arming while the slot is empty; the
    /// render pass emits its load intent on `true`. Later frames get
    /// `false` until [`set`], [`invalidate`] or [`rearm`] runs, no
    /// matter what the dispatcher did with the emitted intent.
    ///
    /// [`set`]: Self::set
    /// [`invalidate`]: Self::invalidate
    /// [`rearm`]: Self::rearm
    pub fn try_fire(&mut self) -> bool {
        if self.data.is_none() && self.armed {
            self.armed = false;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_once_while_empty_and_holds_until_rearmed() {
        let mut slot: LoadSlot<i32> = LoadSlot::default();
        assert!(slot.try_fire(), "a fresh slot fires its first load");
        for _ in 0..3 {
            assert!(!slot.try_fire(), "an unanswered fire must not repeat");
        }
        slot.rearm();
        assert!(slot.try_fire(), "rearm buys exactly one more shot");
        assert!(!slot.try_fire());
    }

    #[test]
    fn a_loaded_slot_never_fires() {
        let mut slot = LoadSlot::default();
        assert!(slot.try_fire());
        slot.set(7);
        assert_eq!(slot.data(), Some(&7));
        assert!(!slot.try_fire(), "a filled cache needs no load");
        // Rearm while full is a no-op observable-wise: still no fire.
        slot.rearm();
        assert!(!slot.try_fire());
    }

    #[test]
    fn invalidate_empties_and_arms_exactly_one_reload() {
        let mut slot = LoadSlot::default();
        assert!(slot.try_fire());
        slot.set(7);
        slot.invalidate();
        assert_eq!(slot.data(), None);
        assert!(slot.try_fire(), "an invalidated cache reloads once");
        assert!(!slot.try_fire(), "and only once");
    }
}
