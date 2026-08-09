use arclain_plugins::types::{PluginExtensionPoint, PluginLayout};

const PLUGINS_README: &str = include_str!("../../../plugins/README.md");
const PLUGINS_CRATE_README: &str = include_str!("../README.md");

fn documented_plugin_manifests(readme: &str) -> impl Iterator<Item = &str> {
    readme
        .split("```toml")
        .skip(1)
        .filter_map(|section| section.split("```").next())
        .filter(|manifest| manifest.contains("[plugin]"))
}

fn accepts_wirt_layout(_: wirt::PluginLayout) {}
fn accepts_wirt_extension(_: wirt::PluginExtensionPoint) {}

#[test]
fn arclain_compatibility_exports_are_the_wirt_types() {
    accepts_wirt_layout(PluginLayout::Single {
        elements: Vec::new(),
    });
    accepts_wirt_extension(PluginExtensionPoint::MainPage);
}

#[test]
fn public_manifest_examples_deserialize_and_validate_current_wirt_abi() {
    let temp_dir = tempfile::tempdir().expect("temporary plugin root should initialize");
    let loader = arclain_plugins::PluginLoader::new(temp_dir.path().to_path_buf())
        .expect("loader should initialize");

    for manifest in documented_plugin_manifests(PLUGINS_README)
        .chain(documented_plugin_manifests(PLUGINS_CRATE_README))
    {
        let manifest = toml::from_str::<arclain_plugins::PluginManifest>(manifest)
            .expect("public manifest example should deserialize");
        loader
            .validate_manifest(&manifest)
            .expect("public manifest example should validate");
    }
}
