#![no_std]

extern crate alloc;

#[macro_use]
extern crate archust_plugin_sdk;

use alloc::string::ToString;
use alloc::vec;
use archust_plugin_sdk::prelude::*;

plugin_metadata!(
    "ui-demo",
    "UI Demo Plugin",
    "0.1.0",
    "Arclain Team",
    "Demonstrates UI capabilities"
);

plugin_init!(|| {
    log(LogLevel::Info, "UI Demo Plugin initialized");
    Ok::<(), ()>(())
});

plugin_cleanup!();

plugin_ui_layout!(|extension_point| {
    if extension_point == PluginExtensionPoint::Sidebar {
        vec![
            PluginUiElement::Label {
                text: "Demo Sidebar UI".to_string(),
                bold: true,
                size: Some(14.0),
            },
            PluginUiElement::Separator,
            PluginUiElement::Row {
                children: vec![
                    PluginUiElement::Label {
                        text: "Status:".to_string(),
                        bold: false,
                        size: None,
                    },
                    PluginUiElement::Space { size: 8.0 },
                    PluginUiElement::Label {
                        text: "Active".to_string(),
                        bold: true,
                        size: None,
                    },
                ],
            },
            PluginUiElement::Button {
                id: "demo_btn".to_string(),
                label: "Click Me".to_string(),
            },
        ]
    } else {
        vec![]
    }
});

plugin_ui_event!(|element_id, value| {
    use alloc::format;
    log(
        LogLevel::Info,
        &format!("UI Event: {} = {:?}", element_id, value),
    );
});
