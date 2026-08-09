pub(crate) use wirt as wirt_crate;

#[allow(dead_code)]
pub const UI_DEMO_COMPONENT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/ui-demo/ui-demo.wasm"
));

#[allow(dead_code)]
pub fn manifest_toml() -> &'static str {
    r#"[wirt]
abi = "0.1.0"

[plugin]
id = "ui-demo"
name = "UI Demo Plugin"
version = "0.2.1"
description = "Demonstrates UI capabilities in the sidebar and plugins page"
author = "Arclain Team"

[capabilities]
network = false
network_domains = []
archive_metadata_read = false
archive_metadata_write = false
archive_modify = false
file_read = false
file_write = false

[rate_limits]
http_requests_per_minute = 60
"#
}

#[path = "../../src/runtime/stub_host.rs"]
#[allow(dead_code)]
pub mod stub_host;
