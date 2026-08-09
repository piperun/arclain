const METADATA_SENTINEL: &str = "arclain-malicious-metadata-sentinel.txt";
const INIT_SENTINEL: &str = "arclain-malicious-init-sentinel.txt";
const LOG_SENTINEL: &str = "arclain-malicious-metadata-global-log";
const MESSAGE_SENTINEL: &str = "arclain-malicious-metadata-show-message";
const STDOUT_SENTINEL: &str = "ARCLAIN_VALIDATION_WASI_STDOUT_SENTINEL_7F3B";
const STDERR_SENTINEL: &str = "ARCLAIN_VALIDATION_WASI_STDERR_SENTINEL_8C4D";

struct Component;

impl wirt_sdk::Guest for Component {
    fn get_metadata() -> wirt_sdk::wirt::plugin::meta::PluginMetadata {
        use std::io::Write;

        let _ = std::io::stdout().write_all(format!("{STDOUT_SENTINEL}\n").as_bytes());
        let _ = std::io::stderr().write_all(format!("{STDERR_SENTINEL}\n").as_bytes());
        let process_context_visible =
            std::env::args_os().next().is_some() || std::env::vars_os().next().is_some();

        let _ = wirt_sdk::create_file(METADATA_SENTINEL, b"metadata side effect");
        wirt_sdk::warn(LOG_SENTINEL);
        wirt_sdk::show_message(MESSAGE_SENTINEL, "must stay sandboxed");

        wirt_sdk::wirt::plugin::meta::PluginMetadata {
            id: if process_context_visible {
                "args-leaked".to_string()
            } else {
                "../evil".to_string()
            },
            name: "Malicious metadata fixture".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: "Arclain security tests".to_string(),
            description: "Probes process context and host side effects before returning an ID"
                .to_string(),
        }
    }

    fn init() {
        if wirt_sdk::create_file(INIT_SENTINEL, b"init side effect").is_err() {
            panic!("metadata validation must never call plugin init");
        }
    }

    fn get_default_rules() -> Vec<wirt_sdk::wirt::plugin::rules::PluginRuleDefinition> {
        use wirt_sdk::wirt::plugin::rules::{
            PluginRuleActions, PluginRuleDefinition, PluginRuleTrigger,
        };

        wirt_sdk::info("neutral-only rule quota probe");
        vec![PluginRuleDefinition {
            name: "quota probe".to_string(),
            category: "neutral category".to_string(),
            description: Some("x".repeat(1024 * 1024)),
            trigger: PluginRuleTrigger {
                filename_pattern: None,
                has_file: None,
                extensions: None,
                min_size: None,
                max_size: None,
                metadata_source: None,
            },
            actions: PluginRuleActions {
                root_folder: None,
                move_files: vec![],
                move_to: None,
                rename_pattern: None,
                organize_content: false,
                delete_original: false,
                use_standard_layout: false,
            },
        }]
    }

    fn get_ui_layout(
        _extension_point: String,
    ) -> wirt_sdk::wirt::plugin::ui::PluginLayout {
        wirt_sdk::wirt::plugin::ui::PluginLayout::Single(vec![])
    }

    fn get_top_tabs() -> Vec<wirt_sdk::wirt::plugin::ui::TopTabConfig> {
        vec![]
    }

    fn on_ui_event(
        _id: String,
        _value: Option<String>,
    ) -> Vec<wirt_sdk::wirt::plugin::ui::PluginAction> {
        vec![]
    }
}

wirt_sdk::export!(Component with_types_in wirt_sdk);
