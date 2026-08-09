struct Component;

impl wirt_sdk::Guest for Component {
    fn get_metadata() -> wirt_sdk::wirt::plugin::meta::PluginMetadata {
        wirt_sdk::wirt::plugin::meta::PluginMetadata {
            id: "wirt-starter".to_string(),
            name: "Wirt Starter".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: "Your Name".to_string(),
            description: "A minimal Wirt plugin".to_string(),
        }
    }

    fn init() {
        wirt_sdk::info("Wirt starter initialized");
    }

    fn get_default_rules() -> Vec<wirt_sdk::wirt::plugin::rules::PluginRuleDefinition> {
        vec![]
    }

    fn get_ui_layout(_: String) -> wirt_sdk::wirt::plugin::ui::PluginLayout {
        wirt_sdk::wirt::plugin::ui::PluginLayout::Single(vec![])
    }

    fn on_ui_event(
        _: String,
        _: Option<String>,
    ) -> Vec<wirt_sdk::wirt::plugin::ui::PluginAction> {
        vec![]
    }

    fn get_top_tabs() -> Vec<wirt_sdk::wirt::plugin::ui::TopTabConfig> {
        vec![]
    }
}

wirt_sdk::export!(Component with_types_in wirt_sdk);
