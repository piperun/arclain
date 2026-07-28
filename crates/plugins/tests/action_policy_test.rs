use arclain_plugins::action_policy::{
    bound_plugin_actions, bound_plugin_actions_with_status, MAX_LIGHTBOX_IMAGES,
    MAX_REQUEST_FETCH_ACTIONS, MAX_TOAST_ACTIONS, MAX_TOAST_MESSAGE_BYTES,
};
use arclain_plugins::types::{PluginAction, ToastLevel};

#[test]
fn guest_action_batch_has_semantic_quotas_and_last_wins_actions() {
    let mut actions = Vec::new();
    for index in 0..(MAX_TOAST_ACTIONS + 3) {
        actions.push(PluginAction::ShowToast {
            message: format!("toast-{index}"),
            level: ToastLevel::Info,
        });
    }
    actions.push(PluginAction::CopyToClipboard {
        text: "old".to_string(),
    });
    actions.push(PluginAction::CopyToClipboard {
        text: "new".to_string(),
    });
    actions.push(PluginAction::SetPageDisplayName {
        name: "old page".to_string(),
    });
    actions.push(PluginAction::SetPageDisplayName {
        name: "new page".to_string(),
    });
    actions.push(PluginAction::RefreshPanel {
        extension_point: "Panel".to_string(),
    });
    actions.push(PluginAction::RefreshPanel {
        extension_point: "MainPage".to_string(),
    });
    for index in 0..(MAX_REQUEST_FETCH_ACTIONS + 3) {
        actions.push(PluginAction::RequestFetch {
            key: format!("dlsite:RJ{index:06}"),
        });
    }

    let bounded = bound_plugin_actions(actions);

    assert_eq!(
        bounded
            .iter()
            .filter(|action| matches!(action, PluginAction::ShowToast { .. }))
            .count(),
        MAX_TOAST_ACTIONS
    );
    assert_eq!(
        bounded
            .iter()
            .filter(|action| matches!(action, PluginAction::RequestFetch { .. }))
            .count(),
        MAX_REQUEST_FETCH_ACTIONS
    );
    assert_eq!(
        bounded
            .iter()
            .filter(|action| matches!(action, PluginAction::RefreshPanel { .. }))
            .count(),
        1
    );
    assert!(bounded
        .iter()
        .any(|action| matches!(action, PluginAction::CopyToClipboard { text } if text == "new")));
    assert!(!bounded
        .iter()
        .any(|action| matches!(action, PluginAction::CopyToClipboard { text } if text == "old")));
    assert!(bounded.iter().any(
        |action| matches!(action, PluginAction::SetPageDisplayName { name } if name == "new page")
    ));
}

#[test]
fn field_truncation_reports_that_the_batch_was_limited() {
    let bounded = bound_plugin_actions_with_status(vec![PluginAction::OpenLightbox {
        images: (0..MAX_LIGHTBOX_IMAGES + 1)
            .map(|index| (format!("key-{index}"), None))
            .collect(),
        start_index: usize::MAX,
        title: None,
    }]);

    assert!(bounded.limited);
    assert_eq!(bounded.actions.len(), 1);
}

#[test]
fn guest_action_batch_rejects_oversized_fields_and_caps_images() {
    let bounded = bound_plugin_actions(vec![
        PluginAction::ShowToast {
            message: "x".repeat(MAX_TOAST_MESSAGE_BYTES + 1),
            level: ToastLevel::Error,
        },
        PluginAction::RequestFetch {
            key: "x".repeat(1024),
        },
        PluginAction::OpenLightbox {
            images: (0..MAX_LIGHTBOX_IMAGES + 10)
                .map(|index| (format!("key-{index}"), Some(format!("https://x/{index}"))))
                .collect(),
            start_index: usize::MAX,
            title: Some("gallery".to_string()),
        },
    ]);

    assert!(!bounded
        .iter()
        .any(|action| matches!(action, PluginAction::ShowToast { .. })));
    assert!(!bounded
        .iter()
        .any(|action| matches!(action, PluginAction::RequestFetch { .. })));
    let PluginAction::OpenLightbox {
        images,
        start_index,
        ..
    } = bounded
        .iter()
        .find(|action| matches!(action, PluginAction::OpenLightbox { .. }))
        .expect("bounded lightbox action")
    else {
        unreachable!()
    };
    assert_eq!(images.len(), MAX_LIGHTBOX_IMAGES);
    assert_eq!(*start_index, MAX_LIGHTBOX_IMAGES - 1);
}
