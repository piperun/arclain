mod support;

use std::path::Path;

use support::stub_host::StubHost;
use wirt::{
    PluginAction, PluginError, PluginExtensionPoint, PluginUiElement, ToastLevel, WasmRuntime,
};

const FACADE_FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/facade-test-fixture/facade-test-fixture.wasm"
);
const DLSITE_FIXTURE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/dlsite-metadata/dlsite-metadata.wasm"
));
const MALICIOUS_METADATA_FIXTURE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../plugins/tests/fixtures/malicious-metadata/malicious-metadata.wasm"
));

#[test]
fn ui_demo_exposes_its_demo_button_on_the_main_page() {
    let runtime = WasmRuntime::new().unwrap();
    let component = runtime
        .load_component_from_bytes(
            "ui-demo".to_string(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../plugins/ui-demo/ui-demo.wasm"
            )),
        )
        .unwrap();
    let mut instance = component.instantiate(StubHost::new()).unwrap();

    let layout = instance
        .get_ui_layout(&PluginExtensionPoint::MainPage)
        .unwrap();
    assert!(layout
        .elements()
        .into_iter()
        .any(|element| matches!(element, PluginUiElement::Button { id, .. } if id == "demo_btn")));
}

#[test]
fn stub_host_runs_a_real_plugin_component_end_to_end() {
    let runtime = WasmRuntime::new().unwrap();
    let component = runtime
        .load_component_from_bytes(
            "ui-demo".to_string(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../plugins/ui-demo/ui-demo.wasm"
            )),
        )
        .unwrap();
    let mut instance = component.instantiate(StubHost::new()).unwrap();

    instance.init().unwrap();
    assert_eq!(instance.get_metadata().unwrap().id, "ui-demo");
    let layout = instance
        .get_ui_layout(&PluginExtensionPoint::MainPage)
        .unwrap();
    assert!(!layout.elements().is_empty());
    assert!(instance.send_ui_event("demo_btn", None).unwrap().is_empty());
}

#[test]
fn file_loading_preserves_the_caller_supplied_component_id() {
    let runtime = WasmRuntime::new().unwrap();
    let component = runtime
        .load_component(
            "runtime-contract".to_string(),
            Path::new(FACADE_FIXTURE_PATH),
        )
        .unwrap();

    assert_eq!(component.id(), "runtime-contract");
    assert!(component.instantiate(StubHost::new()).is_ok());
}

#[test]
fn host_state_can_be_mutated_before_init_and_remains_available_afterward() {
    let runtime = WasmRuntime::new().unwrap();
    let component = runtime
        .load_component(
            "facade-test-fixture".to_string(),
            Path::new(FACADE_FIXTURE_PATH),
        )
        .unwrap();
    let mut instance = component.instantiate(StubHost::new()).unwrap();

    instance.host_state_mut().set_probe("mutated-before-init");
    assert_eq!(instance.host_state().probe(), "mutated-before-init");

    instance.init().unwrap();
    assert_eq!(instance.host_state().probe(), "mutated-before-init");
    assert_eq!(instance.host_state().observed_log_calls(), 1);
}

#[test]
fn generated_guest_call_wrappers_preserve_neutral_results() {
    let runtime = WasmRuntime::new().unwrap();
    let component = runtime
        .load_component(
            "facade-test-fixture".to_string(),
            Path::new(FACADE_FIXTURE_PATH),
        )
        .unwrap();
    let mut instance = component.instantiate(StubHost::new()).unwrap();

    assert_eq!(instance.unavailable_reason(), None);
    instance.init().unwrap();
    assert_eq!(instance.host_state().observed_log_calls(), 1);

    let metadata = instance.get_metadata().unwrap();
    assert_eq!(metadata.id, "facade-test-fixture");
    assert_eq!(metadata.name, "Facade Test Fixture");

    assert!(instance.get_default_rules().unwrap().is_empty());

    let layout = instance
        .get_ui_layout(&PluginExtensionPoint::MainPage)
        .unwrap();
    assert!(layout.elements().into_iter().any(
        |element| matches!(element, PluginUiElement::Button { id, .. } if id == "multi-action")
    ));

    let actions = instance.send_ui_event("multi-action", None).unwrap();
    assert_eq!(
        actions,
        vec![
            PluginAction::ShowToast {
                message: "first".to_string(),
                level: ToastLevel::Info,
            },
            PluginAction::CopyToClipboard {
                text: "second".to_string(),
            },
            PluginAction::SetPageDisplayName {
                name: "third".to_string(),
            },
        ]
    );

    let tabs = instance.get_top_tabs().unwrap();
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].id, "fixture-tab");
    assert_eq!(tabs[0].label, "Fixture");
    assert_eq!(tabs[0].icon, "DATABASE");
    assert_eq!(tabs[0].priority, 250);
    let badge = tabs[0].badge.as_ref().unwrap();
    assert_eq!(badge.count, Some(7));
    assert!(badge.dot);
    assert_eq!(badge.color, "orange");

    instance.cleanup().unwrap();
    assert_eq!(instance.unavailable_reason(), None);
    assert_eq!(instance.get_top_tabs().unwrap(), tabs);
}

#[test]
fn default_rules_wrapper_preserves_neutral_rule_fields() {
    let runtime = WasmRuntime::new().unwrap();
    let component = runtime
        .load_component_from_bytes("dlsite-metadata".to_string(), DLSITE_FIXTURE)
        .unwrap();
    let mut instance = component.instantiate(StubHost::new()).unwrap();

    let rules = instance.get_default_rules().unwrap();
    assert_eq!(rules.len(), 1);
    let rule = &rules[0];
    assert_eq!(rule.name, "DLSite Archive");
    assert_eq!(rule.category, "Game");
    assert_eq!(
        rule.description.as_deref(),
        Some("Organizes DLSite game archives logic")
    );
    assert_eq!(rule.trigger.metadata_source.as_deref(), Some("dlsite"));
    assert_eq!(
        rule.actions.root_folder.as_deref(),
        Some("$maker_name/$work_name")
    );
    assert!(rule.actions.organize_content);
    assert!(rule.actions.use_standard_layout);
}

#[test]
fn neutral_only_default_rule_data_counts_toward_the_terminal_result_quota() {
    let runtime = WasmRuntime::new().unwrap();
    let component = runtime
        .load_component_from_bytes("neutral-rule-quota".to_string(), MALICIOUS_METADATA_FIXTURE)
        .unwrap();
    let mut instance = component.instantiate(StubHost::new()).unwrap();

    let first = instance.get_default_rules().unwrap_err();
    assert!(matches!(
        first,
        PluginError::ResourceLimit { ref reason } if reason == "plugin result quota exceeded"
    ));
    assert_eq!(
        instance.unavailable_reason(),
        Some("plugin result quota exceeded")
    );
    assert_eq!(instance.host_state().observed_log_calls(), 1);

    let second = instance.get_default_rules().unwrap_err();
    assert!(matches!(
        second,
        PluginError::Unavailable(ref reason) if reason == "plugin result quota exceeded"
    ));
    assert_eq!(instance.host_state().observed_log_calls(), 1);
}
