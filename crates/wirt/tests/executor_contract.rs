use std::sync::atomic::{AtomicUsize, Ordering};

use wirt::{
    ExecutorRequest, ExecutorResponse, PluginAction, PluginExtensionPoint, PluginId, PluginLayout,
    PluginMetadata, PluginRuleActions, PluginRuleDefinition, PluginRuleTrigger, PluginUiElement,
    Result, TextRole, TopTabConfig, ValidatedExecutorRequest, WirtExecutor, WirtExecutorBackend,
    MAX_EXECUTOR_MESSAGE_BYTES,
};

fn rule() -> PluginRuleDefinition {
    PluginRuleDefinition {
        name: "archives".to_string(),
        category: "media".to_string(),
        description: Some("Archive files".to_string()),
        trigger: PluginRuleTrigger {
            filename_pattern: Some("*.zip".to_string()),
            has_file: None,
            extensions: Some(vec!["zip".to_string()]),
            min_size: None,
            max_size: None,
            metadata_source: None,
        },
        actions: PluginRuleActions {
            root_folder: None,
            move_files: Vec::new(),
            move_to: None,
            rename_pattern: None,
            organize_content: false,
            delete_original: false,
            use_standard_layout: true,
        },
    }
}

fn metadata() -> PluginMetadata {
    PluginMetadata {
        id: "contract-test".to_string(),
        name: "Contract Test".to_string(),
        version: "1.0.0".to_string(),
        author: "Wirt".to_string(),
        description: "Executor contract".to_string(),
    }
}

#[test]
fn every_executor_message_round_trips_as_neutral_json() {
    let requests = vec![
        ExecutorRequest::Init,
        ExecutorRequest::Metadata,
        ExecutorRequest::DefaultRules,
        ExecutorRequest::UiLayout {
            extension_point: PluginExtensionPoint::Dialog("approval".to_string()),
        },
        ExecutorRequest::UiEvent {
            id: "install".to_string(),
            value: Some("approved".to_string()),
        },
        ExecutorRequest::TopTabs,
        ExecutorRequest::Cleanup,
    ];
    for request in requests {
        let bytes = serde_json::to_vec(&request).expect("serialize request");
        let decoded: ExecutorRequest = serde_json::from_slice(&bytes).expect("deserialize request");
        assert_eq!(decoded, request);
        request.validate().expect("bounded request");
    }

    let responses = vec![
        ExecutorResponse::Empty,
        ExecutorResponse::Metadata(metadata()),
        ExecutorResponse::Rules(vec![rule()]),
        ExecutorResponse::Layout(PluginLayout::Single {
            elements: vec![PluginUiElement::Label {
                text: "ready".to_string(),
                role: TextRole::Body,
            }],
        }),
        ExecutorResponse::Actions(vec![PluginAction::None]),
        ExecutorResponse::TopTabs(vec![TopTabConfig {
            id: "main".to_string(),
            label: "Main".to_string(),
            icon: "plugin".to_string(),
            badge: None,
            priority: 1,
        }]),
    ];
    for response in responses {
        let bytes = serde_json::to_vec(&response).expect("serialize response");
        let decoded: ExecutorResponse =
            serde_json::from_slice(&bytes).expect("deserialize response");
        assert_eq!(decoded, response);
        response.validate().expect("bounded response");
    }
}

struct CountingExecutor(AtomicUsize);

impl WirtExecutorBackend for CountingExecutor {
    fn execute_validated(
        &self,
        _plugin_id: &PluginId,
        _request: ValidatedExecutorRequest,
    ) -> Result<ExecutorResponse> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(ExecutorResponse::Empty)
    }
}

#[test]
fn oversized_requests_fail_before_executor_lookup_or_guest_entry() {
    let executor = CountingExecutor(AtomicUsize::new(0));
    let plugin_id = PluginId::parse("contract-test").expect("valid plugin id");
    let request = ExecutorRequest::UiEvent {
        id: "x".repeat(MAX_EXECUTOR_MESSAGE_BYTES),
        value: None,
    };

    let error = executor.execute(&plugin_id, request).unwrap_err();
    assert_eq!(executor.0.load(Ordering::Relaxed), 0);
    assert_eq!(
        error.to_string(),
        "Plugin execution failed: executor request limit exceeded"
    );
}

#[test]
fn oversized_responses_fail_before_crossing_the_executor_boundary() {
    let response = ExecutorResponse::Layout(PluginLayout::Single {
        elements: vec![PluginUiElement::Label {
            text: "x".repeat(MAX_EXECUTOR_MESSAGE_BYTES),
            role: TextRole::Body,
        }],
    });

    assert_eq!(
        response.validate().unwrap_err().to_string(),
        "Plugin execution failed: executor response limit exceeded"
    );
}

struct OversizedResponseExecutor;

impl WirtExecutorBackend for OversizedResponseExecutor {
    fn execute_validated(
        &self,
        _plugin_id: &PluginId,
        _request: ValidatedExecutorRequest,
    ) -> Result<ExecutorResponse> {
        Ok(ExecutorResponse::Layout(PluginLayout::Single {
            elements: vec![PluginUiElement::Label {
                text: "x".repeat(MAX_EXECUTOR_MESSAGE_BYTES),
                role: TextRole::Body,
            }],
        }))
    }
}

#[test]
fn executor_validates_backend_responses_before_returning_them() {
    let plugin_id = PluginId::parse("contract-test").expect("valid plugin id");
    let error = OversizedResponseExecutor
        .execute(&plugin_id, ExecutorRequest::Metadata)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Plugin execution failed: executor response limit exceeded"
    );
}

#[test]
fn response_limit_includes_the_serialized_message_envelope() {
    let mut low = 0usize;
    let mut high = MAX_EXECUTOR_MESSAGE_BYTES;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        let layout = PluginLayout::Single {
            elements: vec![PluginUiElement::Label {
                text: "x".repeat(middle),
                role: TextRole::Body,
            }],
        };
        if serde_json::to_vec(&layout).unwrap().len() <= MAX_EXECUTOR_MESSAGE_BYTES {
            low = middle;
        } else {
            high = middle - 1;
        }
    }

    let response = ExecutorResponse::Layout(PluginLayout::Single {
        elements: vec![PluginUiElement::Label {
            text: "x".repeat(low),
            role: TextRole::Body,
        }],
    });
    assert!(serde_json::to_vec(&response).unwrap().len() > MAX_EXECUTOR_MESSAGE_BYTES);
    assert_eq!(
        response.validate().unwrap_err().to_string(),
        "Plugin execution failed: executor response limit exceeded"
    );
}
