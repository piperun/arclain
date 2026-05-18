//! Per-tab WASM PluginInstance pool. Lazy-spawn / eager-drop.
//!
//! In Phase 2a this is an empty struct so `TabState` can hold the
//! field; the actual pool plumbing lands in Phase 2c when per-tab
//! plugin instances replace the global `PluginManager` pool.

#[derive(Debug, Default)]
pub struct TabPluginPool {
    // Phase 2c will fill this in.
}
