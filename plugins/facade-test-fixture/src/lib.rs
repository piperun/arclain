//! Deterministic plugin fixture used only by `arclain_app`'s own facade
//! tests (`crates/app/tests/plugin_sessions.rs`). Not a user-facing
//! plugin: it exists purely to exercise three brief-mandated behaviors a
//! "real" demo plugin (`ui-demo`, whose `on-ui-event` always returns an
//! empty action list) cannot exercise:
//!
//! - **Crash containment**: the `"trigger-trap"` button's `on-ui-event`
//!   handler panics unconditionally. Under `panic = "abort"` (this
//!   crate's release profile, matching every other plugin fixture in
//!   this workspace) that compiles to a WASM `unreachable` trap, which
//!   wasmtime surfaces as an ordinary `Result::Err` to the host -- never
//!   a host-process panic. A test can dispatch this action, observe the
//!   resulting `ApplicationError`, and then dispatch a *different*
//!   action against the same session to prove the instance and its
//!   store are still usable afterward.
//! - **Action ordering**: the `"multi-action"` button's handler returns
//!   three different actions in one response (`ShowToast`,
//!   `CopyToClipboard`, `SetPageDisplayName`), in a fixed order, so a
//!   test can assert the resulting `PluginHostIntentDto` list preserves
//!   that order.
//! - **Refresh coalescing**: the `"multi-refresh"` button's handler
//!   returns three `RefreshPanel` actions in one response.
//!   `get-ui-layout`'s own call count is baked into every returned
//!   `MainPage` layout (`"layout-call-{n}"`), so a test can dispatch this
//!   action and confirm the resulting document's label advanced by
//!   exactly one call, not three.
//! - **Plugin chrome**: `get-top-tabs` returns exactly one tab, with a
//!   badge, at a fixed priority, so a test can read `plugin_chrome` back
//!   and assert every mirrored field. `ui-demo` registers no tabs at all.
//! - **Network log**: `init` writes exactly one `log-network-activity`
//!   line, so a test can read `plugin_network_log` back without the
//!   plugin needing the network capability or a live server. `init` is
//!   the only deterministic write point: it runs exactly once per load,
//!   whereas anything on a UI path would make the line count depend on
//!   how many other tests rendered first.

use archust_plugin_sdk::{info, log_network_activity};
use std::sync::atomic::{AtomicU32, Ordering};

/// The single network-log line [`Component::init`] writes. Named here so
/// `crates/app`'s own test can assert on it without repeating the string.
const INIT_NETWORK_LOG_LINE: &str = "facade-test-fixture: initialized";

/// Incremented on every `get-ui-layout` call for `"MainPage"`. Backed by
/// this WASM instance's own linear memory, so it persists across calls
/// for as long as the host keeps this plugin instance alive (the host
/// never re-instantiates a running plugin between calls) -- exactly the
/// property the refresh-coalescing test needs to observe.
static MAIN_PAGE_LAYOUT_CALLS: AtomicU32 = AtomicU32::new(0);

struct Component;

impl archust_plugin_sdk::Guest for Component {
    fn get_metadata() -> archust_plugin_sdk::arclain::plugin::meta::PluginMetadata {
        archust_plugin_sdk::arclain::plugin::meta::PluginMetadata {
            id: "facade-test-fixture".to_string(),
            name: "Facade Test Fixture".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: "Arclain Team".to_string(),
            description: "Deterministic plugin used only by arclain_app's own facade tests"
                .to_string(),
        }
    }

    fn init() {
        info("Facade test fixture initialized");
        log_network_activity(INIT_NETWORK_LOG_LINE);
    }

    fn get_default_rules() -> Vec<archust_plugin_sdk::arclain::plugin::rules::PluginRuleDefinition>
    {
        vec![]
    }

    fn get_ui_layout(
        extension_point: String,
    ) -> archust_plugin_sdk::arclain::plugin::ui::PluginLayout {
        use archust_plugin_sdk::arclain::plugin::ui::*;

        match extension_point.as_str() {
            "MainPage" => {
                let call_number = MAIN_PAGE_LAYOUT_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
                PluginLayout::Single(vec![
                    UiElement::Label(LabelConfig {
                        text: format!("layout-call-{call_number}"),
                        bold: false,
                        size: None,
                    }),
                    UiElement::Button(ButtonConfig {
                        id: "trigger-trap".to_string(),
                        label: "Trigger Trap".to_string(),
                        action: None,
                    }),
                    UiElement::Button(ButtonConfig {
                        id: "multi-action".to_string(),
                        label: "Multi Action".to_string(),
                        action: None,
                    }),
                    UiElement::Button(ButtonConfig {
                        id: "multi-refresh".to_string(),
                        label: "Multi Refresh".to_string(),
                        action: None,
                    }),
                ])
            }
            _ => PluginLayout::Single(vec![]),
        }
    }

    fn get_top_tabs() -> Vec<archust_plugin_sdk::arclain::plugin::ui::TopTabConfig> {
        use archust_plugin_sdk::arclain::plugin::ui::{BadgeConfig, TopTabConfig};

        // Every field distinct and constant: a count that is not the
        // priority, a `dot` that is not the default, and a colour the
        // renderer maps rather than passes through -- so a mirror test
        // that transposed two fields would fail rather than pass by
        // coincidence.
        vec![TopTabConfig {
            id: "fixture-tab".to_string(),
            label: "Fixture".to_string(),
            icon: "DATABASE".to_string(),
            badge: Some(BadgeConfig {
                count: Some(7),
                dot: true,
                color: "orange".to_string(),
            }),
            priority: 250,
        }]
    }

    fn on_ui_event(
        id: String,
        _value: Option<String>,
    ) -> Vec<archust_plugin_sdk::arclain::plugin::ui::PluginAction> {
        use archust_plugin_sdk::arclain::plugin::ui::*;

        match id.as_str() {
            "trigger-trap" => {
                panic!("facade-test-fixture: intentional trap for crash-containment tests")
            }
            "multi-action" => vec![
                PluginAction::ShowToast(ToastConfig {
                    message: "first".to_string(),
                    level: ToastLevel::Info,
                }),
                PluginAction::CopyToClipboard("second".to_string()),
                PluginAction::SetPageDisplayName("third".to_string()),
            ],
            "multi-refresh" => vec![
                PluginAction::RefreshPanel("MainPage".to_string()),
                PluginAction::RefreshPanel("MainPage".to_string()),
                PluginAction::RefreshPanel("MainPage".to_string()),
            ],
            _ => vec![],
        }
    }
}

archust_plugin_sdk::export!(Component with_types_in archust_plugin_sdk);
