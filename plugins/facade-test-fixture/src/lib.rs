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
//! - **Settings persistence**: the `Panel` extension point's
//!   `"remember"` button writes a setting from inside `on-ui-event`, and
//!   the `Panel` layout labels whatever that setting currently holds. A
//!   host test can therefore write a value through one application and
//!   read it back through a *freshly bootstrapped* one, where it can only
//!   be present if the host pulled it out of the instance and stored it.
//!   `init` writes a second setting -- a load counter derived from its
//!   own persisted value -- so the same round trip is covered for a guest
//!   call that is not a UI event. No other fixture writes a setting at
//!   all, which is why nothing caught a host that never pulled.
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

/// The settings key the `"remember"` button writes and the `Panel` layout
/// reads back -- the two halves of the settings round trip.
const REMEMBERED_SETTING_KEY: &str = "remembered-code";

/// How many times this plugin has been loaded, written by
/// [`Component::init`] and reported by the `Panel` layout.
///
/// Deliberately *derived from its own previous value*: a key `init` wrote
/// a constant into would read the same on every load whether or not the
/// host had persisted anything, and would prove nothing. A counter only
/// reaches two if the first load's write was pulled out of the instance
/// and stored -- which is what makes it a test of the host rather than of
/// the guest.
const LOAD_COUNT_SETTING_KEY: &str = "load-count";

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
        // A settings write from a guest call that is *not* `on-ui-event`
        // -- the shape a plugin recording its own state or seeding its
        // defaults at load takes. The host seeds stored settings into the
        // instance before calling `init`, so this reads back whatever the
        // last load persisted.
        let loads = archust_plugin_sdk::arclain::plugin::host::get_setting(LOAD_COUNT_SETTING_KEY)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
            + 1;
        archust_plugin_sdk::set_setting(LOAD_COUNT_SETTING_KEY, &loads.to_string());
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
            // The settings round trip. The label reports what this guest
            // currently holds for `REMEMBERED_SETTING_KEY`, so a host test
            // can read the value back out of the plugin -- including from
            // a *freshly bootstrapped* application, where the only way the
            // value can be there is if it was persisted and seeded back
            // in. `on-ui-event` is the write side (see below), which is
            // exactly where a real plugin's settings form writes from.
            "Panel" => PluginLayout::Single(vec![
                UiElement::Label(LabelConfig {
                    text: format!(
                        "remembered:{}",
                        archust_plugin_sdk::arclain::plugin::host::get_setting(
                            REMEMBERED_SETTING_KEY
                        )
                        .unwrap_or_else(|| "unset".to_string()),
                    ),
                    bold: false,
                    size: None,
                }),
                UiElement::Label(LabelConfig {
                    text: format!(
                        "loads:{}",
                        archust_plugin_sdk::arclain::plugin::host::get_setting(
                            LOAD_COUNT_SETTING_KEY
                        )
                        .unwrap_or_else(|| "0".to_string()),
                    ),
                    bold: false,
                    size: None,
                }),
                UiElement::Button(ButtonConfig {
                    id: "remember".to_string(),
                    label: "Remember".to_string(),
                    action: None,
                }),
            ]),
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
        value: Option<String>,
    ) -> Vec<archust_plugin_sdk::arclain::plugin::ui::PluginAction> {
        use archust_plugin_sdk::arclain::plugin::ui::*;

        match id.as_str() {
            // A settings write from inside `on-ui-event` -- the shape
            // every real settings form uses (dlsite-metadata's toggles do
            // exactly this). The host only learns about it through the
            // instance's dirty bit, so a host that never pulls the
            // settings after the event silently loses the write; the
            // refresh makes the guest's own view of it observable in the
            // very same dispatch.
            "remember" => {
                archust_plugin_sdk::set_setting(
                    REMEMBERED_SETTING_KEY,
                    value.as_deref().unwrap_or_default(),
                );
                vec![PluginAction::RefreshPanel("Panel".to_string())]
            }
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
