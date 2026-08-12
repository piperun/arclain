use std::collections::BTreeSet;
use wasmtime::component::Component;
use wasmtime::{Config, Engine};

const EXPECTED_PLUGIN_IMPORTS: [&str; 4] = [
    "wirt:plugin/host@0.3.0",
    "wirt:plugin/meta@0.3.0",
    "wirt:plugin/rules@0.3.0",
    "wirt:plugin/ui@0.3.0",
];

fn non_wasi_component_imports(component: &[u8]) -> BTreeSet<String> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).expect("component engine should initialize");
    let component = Component::new(&engine, component).expect("component should parse");

    component
        .component_type()
        .imports(&engine)
        .map(|(name, _)| name.to_owned())
        .filter(|name| !name.starts_with("wasi:"))
        .collect()
}

#[test]
fn arbitrary_payload_strings_do_not_spoof_component_imports() {
    let spoofed_component = br#"
        (component
            (import "wrong:plugin/host@9.9.9" (func))
            (core module
                (memory 1)
                (data (i32.const 0)
                    "wirt:plugin/host@0.3.0"
                    "wirt:plugin/meta@0.3.0"
                    "wirt:plugin/rules@0.3.0"
                    "wirt:plugin/ui@0.3.0")))
    "#;

    assert_eq!(
        non_wasi_component_imports(spoofed_component),
        BTreeSet::from(["wrong:plugin/host@9.9.9".to_string()]),
    );
}

#[test]
fn tracked_malicious_fixture_uses_versioned_wirt_abi() {
    let fixture = include_bytes!("fixtures/malicious-metadata/malicious-metadata.wasm");
    let expected = EXPECTED_PLUGIN_IMPORTS.map(str::to_owned).into();

    assert_eq!(non_wasi_component_imports(fixture), expected);
}
