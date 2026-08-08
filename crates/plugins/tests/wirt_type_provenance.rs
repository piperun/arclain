use arclain_plugins::types::{PluginExtensionPoint, PluginLayout};

fn accepts_wirt_layout(_: wirt::PluginLayout) {}
fn accepts_wirt_extension(_: wirt::PluginExtensionPoint) {}

#[test]
fn arclain_compatibility_exports_are_the_wirt_types() {
    accepts_wirt_layout(PluginLayout::Single {
        elements: Vec::new(),
    });
    accepts_wirt_extension(PluginExtensionPoint::MainPage);
}
