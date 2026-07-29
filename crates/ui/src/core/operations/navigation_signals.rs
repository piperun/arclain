//! Folder navigation inside the active tab's archive.
//!
//! Each function walks the active tab's [`TabListing`] cursor and
//! publishes the result back onto its `listing` signal, so a caller never
//! reaches into the listing state itself. Every one reports whether the
//! cursor actually moved -- callers use that to decide whether to refresh
//! the browser's rows at all.
//!
//! Moving the cursor re-points the tab's [`ListEntriesRequest`] at the
//! new directory and drops the page that answered the old one; see
//! [`TabListing`] for that bookkeeping.
//!
//! [`ListEntriesRequest`]: arclain_app::archive::ListEntriesRequest
//! [`TabListing`]: crate::core::tabs::TabListing

use crate::core::signals::AppSignals;
use crate::core::tabs::TabListing;

/// Runs one navigation against the active tab's listing, publishing the
/// result only when the cursor moved.
///
/// `Signal::update` holds its write lock for the whole closure, so the
/// read-navigate-write sequence is atomic against any other writer of the
/// same listing -- unlike a `get()`, mutate, `set()` round trip, where two
/// concurrent navigations each read the same snapshot and one silently
/// discards the other's move.
fn navigate(signals: &AppSignals, navigation: impl FnOnce(&mut TabListing) -> bool) -> bool {
    let tab = signals.tabs.get().active().clone();
    let mut moved = false;
    tab.listing.update(|listing| {
        moved = navigation(listing);
    });
    moved
}

/// Descend into `folder`, relative to the directory currently shown.
pub fn navigate_to(signals: &AppSignals, folder: &str) -> bool {
    navigate(signals, |listing| listing.descend(folder))
}

/// Jump to `path`, interpreted from the archive root.
pub fn navigate_to_absolute(signals: &AppSignals, path: &str) -> bool {
    navigate(signals, |listing| listing.go_to(path))
}

/// Step back through the folder history, then out to the archive root.
pub fn navigate_back(signals: &AppSignals) -> bool {
    navigate(signals, TabListing::back)
}

/// Step forward through whatever [`navigate_back`] walked out of.
pub fn navigate_forward(signals: &AppSignals) -> bool {
    navigate(signals, TabListing::forward)
}

/// Move to the parent folder.
pub fn navigate_up(signals: &AppSignals) -> bool {
    navigate(signals, TabListing::up)
}

/// Return the active tab to the archive root with an empty history --
/// what opening a different archive into the tab starts from.
pub fn reset_navigation(signals: &AppSignals) {
    signals
        .tabs
        .get()
        .active()
        .listing
        .set(TabListing::default());
}
