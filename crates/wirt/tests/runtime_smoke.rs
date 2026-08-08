mod support;

use support::stub_host::StubHost;
use wirt::{PluginExtensionPoint, PluginUiElement, WasmRuntime};

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
