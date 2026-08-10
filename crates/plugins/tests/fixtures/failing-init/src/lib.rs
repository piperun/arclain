struct Component;

impl wirt_sdk::Guest for Component {
    fn get_metadata() -> wirt_sdk::wirt::plugin::meta::PluginMetadata {
        wirt_sdk::wirt::plugin::meta::PluginMetadata {
            id: "failing-init".to_string(),
            name: "Failing Init Fixture".to_string(),
            version: "0.1.0".to_string(),
            author: "Arclain security tests".to_string(),
            description: "Package-valid guest that traps during initialization".to_string(),
        }
    }

    fn init() {
        wirt_sdk::info("failing-init fixture reached init");
        panic!("intentional init failure");
    }

    fn get_default_rules() -> Vec<wirt_sdk::wirt::plugin::rules::PluginRuleDefinition> {
        Vec::new()
    }

    fn get_ui_layout(
        _extension_point: String,
    ) -> wirt_sdk::wirt::plugin::ui::PluginLayout {
        wirt_sdk::wirt::plugin::ui::PluginLayout::Single(Vec::new())
    }

    fn get_top_tabs() -> Vec<wirt_sdk::wirt::plugin::ui::TopTabConfig> {
        Vec::new()
    }

    fn on_ui_event(
        _id: String,
        _value: Option<String>,
    ) -> Vec<wirt_sdk::wirt::plugin::ui::PluginAction> {
        Vec::new()
    }
}

wirt_sdk::export!(Component with_types_in wirt_sdk);
