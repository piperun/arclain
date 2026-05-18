//! Navigation operations using reactive signals
//!
//! This module replaces direct manipulation of AppState.navigation with signal updates.

use crate::core::signals::AppSignals;
use arclain_core::archive::NavigationState;

/// Navigate to a specific folder within the current archive
pub fn navigate_to(signals: &AppSignals, folder: &str) {
    let tab = signals.tabs.get().active().clone();
    let mut nav = tab.navigation.get();
    nav.navigate_to(folder);
    tab.navigation.set(nav);
}

/// Navigate to an absolute path within the current archive
pub fn navigate_to_absolute(signals: &AppSignals, path: &str) {
    let tab = signals.tabs.get().active().clone();
    let mut nav = tab.navigation.get();
    nav.navigate_to_absolute(path);
    tab.navigation.set(nav);
}

/// Navigate back in history
pub fn navigate_back(signals: &AppSignals) -> bool {
    let tab = signals.tabs.get().active().clone();
    let mut nav = tab.navigation.get();
    if nav.navigate_back() {
        tab.navigation.set(nav);
        true
    } else {
        false
    }
}

/// Navigate forward in history
pub fn navigate_forward(signals: &AppSignals) -> bool {
    let tab = signals.tabs.get().active().clone();
    let mut nav = tab.navigation.get();
    if nav.navigate_forward() {
        tab.navigation.set(nav);
        true
    } else {
        false
    }
}

/// Navigate up one level
pub fn navigate_up(signals: &AppSignals) -> bool {
    let tab = signals.tabs.get().active().clone();
    let mut nav = tab.navigation.get();
    if nav.navigate_up() {
        tab.navigation.set(nav);
        true
    } else {
        false
    }
}

/// Reset navigation state (e.g. when opening new archive)
#[allow(dead_code)]
pub fn reset_navigation(signals: &AppSignals) {
    signals.tabs.get().active().navigation.set(NavigationState::new());
}
