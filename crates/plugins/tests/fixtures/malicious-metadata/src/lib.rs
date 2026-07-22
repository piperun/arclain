const METADATA_SENTINEL: &str = "arclain-malicious-metadata-sentinel.txt";
const INIT_SENTINEL: &str = "arclain-malicious-init-sentinel.txt";
const LOG_SENTINEL: &str = "arclain-malicious-metadata-global-log";
const MESSAGE_SENTINEL: &str = "arclain-malicious-metadata-show-message";

struct Component;

impl archust_plugin_sdk::Guest for Component {
    fn get_metadata() -> archust_plugin_sdk::arclain::plugin::meta::PluginMetadata {
        let _ = archust_plugin_sdk::create_file(METADATA_SENTINEL, b"metadata side effect");
        archust_plugin_sdk::warn(LOG_SENTINEL);
        archust_plugin_sdk::show_message(MESSAGE_SENTINEL, "must stay sandboxed");

        archust_plugin_sdk::arclain::plugin::meta::PluginMetadata {
            id: "../evil".to_string(),
            name: "Malicious metadata fixture".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: "Arclain security tests".to_string(),
            description: "Attempts host side effects before returning an invalid ID".to_string(),
        }
    }

    fn init() {
        if archust_plugin_sdk::create_file(INIT_SENTINEL, b"init side effect").is_err() {
            panic!("metadata validation must never call plugin init");
        }
    }

    fn get_default_rules() -> Vec<archust_plugin_sdk::arclain::plugin::rules::PluginRuleDefinition>
    {
        vec![]
    }

    fn get_ui_layout(
        _extension_point: String,
    ) -> archust_plugin_sdk::arclain::plugin::ui::PluginLayout {
        archust_plugin_sdk::arclain::plugin::ui::PluginLayout::Single(vec![])
    }

    fn get_top_tabs() -> Vec<archust_plugin_sdk::arclain::plugin::ui::TopTabConfig> {
        vec![]
    }

    fn on_ui_event(
        _id: String,
        _value: Option<String>,
    ) -> Vec<archust_plugin_sdk::arclain::plugin::ui::PluginAction> {
        vec![]
    }
}

archust_plugin_sdk::export!(Component with_types_in archust_plugin_sdk);
