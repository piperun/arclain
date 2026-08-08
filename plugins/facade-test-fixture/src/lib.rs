//! Deterministic plugin fixture used only by `arclain_app`'s own facade
//! tests (`crates/app/tests/plugin_sessions.rs`). Not a user-facing
//! plugin: it exists purely to exercise three brief-mandated behaviors a
//! "real" demo plugin (`ui-demo`, whose `on-ui-event` always returns an
//! empty action list) cannot exercise:
//!
//! - **Crash containment**: the `"trigger-trap"` button's `on-ui-event`
//!   handler writes a setting and then panics unconditionally. Under
//!   `panic = "abort"` (this
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
//! - **A dialog with real content**: `"Dialog:fixture-dialog"` returns a
//!   layout carrying its own call counter (`"dialog-layout-call-{n}"`),
//!   an action button, and the two declarative navigation buttons a
//!   dialog host has to resolve without ever forwarding them to this
//!   guest (`CloseDialog`, `OpenPage`). `ui-demo` implements no dialog at
//!   all, so egui's dialog host had no fixture to draw before this.
//! - **A page whose initialization is observable**:
//!   `"Page:fixture-page"` and `"Page:fixture-child"` bake their shared
//!   `get-ui-layout` call count into the document. Opening a page first
//!   reads call 1; dispatching `__page_init` must force the application to
//!   return call 2, so a frontend can prove it never draws the pre-init
//!   document. The pages also expose ordinary interaction and typed
//!   open/close-page buttons for host lifecycle coverage.
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

/// The dialog counterpart of [`MAIN_PAGE_LAYOUT_CALLS`], counted
/// separately so a test can tell "the dialog host re-entered the guest"
/// apart from any main-page read another test in the same process made.
static DIALOG_LAYOUT_CALLS: AtomicU32 = AtomicU32::new(0);

/// The page counterpart of [`MAIN_PAGE_LAYOUT_CALLS`]. Both fixture
/// pages share it so push/pop tests can prove exactly when the host
/// re-entered `get-ui-layout`.
static PAGE_LAYOUT_CALLS: AtomicU32 = AtomicU32::new(0);

/// The dialog id this fixture answers with real content. Anything else
/// falls through to the empty layout, the way every unimplemented
/// extension point does.
const FIXTURE_DIALOG: &str = "Dialog:fixture-dialog";
const FIXTURE_PAGE: &str = "Page:fixture-page";
const FIXTURE_CHILD_PAGE: &str = "Page:fixture-child";

struct Component;

impl archust_plugin_sdk::Guest for Component {
    fn get_metadata() -> archust_plugin_sdk::wirt::plugin::meta::PluginMetadata {
        archust_plugin_sdk::wirt::plugin::meta::PluginMetadata {
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
        let loads = archust_plugin_sdk::wirt::plugin::host::get_setting(LOAD_COUNT_SETTING_KEY)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
            + 1;
        archust_plugin_sdk::set_setting(LOAD_COUNT_SETTING_KEY, &loads.to_string());
    }

    fn get_default_rules() -> Vec<archust_plugin_sdk::wirt::plugin::rules::PluginRuleDefinition> {
        vec![]
    }

    fn get_ui_layout(
        extension_point: String,
    ) -> archust_plugin_sdk::wirt::plugin::ui::PluginLayout {
        use archust_plugin_sdk::wirt::plugin::ui::*;

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
                        archust_plugin_sdk::wirt::plugin::host::get_setting(REMEMBERED_SETTING_KEY)
                            .unwrap_or_else(|| "unset".to_string()),
                    ),
                    bold: false,
                    size: None,
                }),
                UiElement::Label(LabelConfig {
                    text: format!(
                        "loads:{}",
                        archust_plugin_sdk::wirt::plugin::host::get_setting(LOAD_COUNT_SETTING_KEY)
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
            FIXTURE_DIALOG => {
                let call_number = DIALOG_LAYOUT_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
                PluginLayout::Single(vec![
                    UiElement::Label(LabelConfig {
                        text: format!("dialog-layout-call-{call_number}"),
                        bold: false,
                        size: None,
                    }),
                    // Reuses `on-ui-event`'s `"multi-action"` handler, so a
                    // press from a dialog is observable through the same
                    // three ordered actions the main page's is.
                    UiElement::Button(ButtonConfig {
                        id: "multi-action".to_string(),
                        label: "Dialog Multi Action".to_string(),
                        action: None,
                    }),
                    // The two declarative navigations a host must resolve
                    // itself. `on-ui-event` deliberately has no arm for
                    // either id: reaching it would mean the host forwarded
                    // navigation to the guest, which is the bug the typed
                    // button action exists to make impossible.
                    UiElement::Button(ButtonConfig {
                        id: "dialog-close".to_string(),
                        label: "Dialog Close".to_string(),
                        action: Some(ButtonAction::CloseDialog),
                    }),
                    UiElement::Button(ButtonConfig {
                        id: "dialog-open-page".to_string(),
                        label: "Dialog Open Page".to_string(),
                        action: Some(ButtonAction::OpenPage("fixture-page".to_string())),
                    }),
                ])
            }
            FIXTURE_PAGE | FIXTURE_CHILD_PAGE => {
                let call_number = PAGE_LAYOUT_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
                PluginLayout::Single(vec![
                    UiElement::Label(LabelConfig {
                        text: format!("page-layout-call-{call_number}"),
                        bold: false,
                        size: None,
                    }),
                    UiElement::Label(LabelConfig {
                        text: extension_point
                            .strip_prefix("Page:")
                            .unwrap_or_default()
                            .to_string(),
                        bold: false,
                        size: None,
                    }),
                    UiElement::Button(ButtonConfig {
                        id: "multi-action".to_string(),
                        label: "Page Multi Action".to_string(),
                        action: None,
                    }),
                    UiElement::Button(ButtonConfig {
                        id: "page-open-child".to_string(),
                        label: "Page Open Child".to_string(),
                        action: Some(ButtonAction::OpenPage("fixture-child".to_string())),
                    }),
                    UiElement::Button(ButtonConfig {
                        id: "page-close".to_string(),
                        label: "Page Close".to_string(),
                        action: Some(ButtonAction::ClosePage),
                    }),
                ])
            }
            _ => PluginLayout::Single(vec![]),
        }
    }

    fn get_top_tabs() -> Vec<archust_plugin_sdk::wirt::plugin::ui::TopTabConfig> {
        use archust_plugin_sdk::wirt::plugin::ui::{BadgeConfig, TopTabConfig};

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
    ) -> Vec<archust_plugin_sdk::wirt::plugin::ui::PluginAction> {
        use archust_plugin_sdk::wirt::plugin::ui::*;

        match id.as_str() {
            "__page_init" => vec![PluginAction::SetPageDisplayName(
                value
                    .map(|page| format!("Fixture Page ({page})"))
                    .unwrap_or_else(|| "Fixture Page".to_string()),
            )],
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
                // Writes *before* trapping, on purpose. The write lands in
                // host-side instance state, so it survives the trap the
                // next line causes -- which makes this the case that
                // proves a host pulls settings on the failure path and not
                // only when a dispatch succeeds.
                archust_plugin_sdk::set_setting(REMEMBERED_SETTING_KEY, "trapped");
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
