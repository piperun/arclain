//! Serializable execution messages for the Wirt host boundary.

use crate::runtime::quota::{
    validate_actions_result, validate_layout_result, validate_serialized_result,
    validate_top_tabs_result, ResultValidationError,
};
use crate::{
    PluginAction, PluginError, PluginExtensionPoint, PluginId, PluginLayout, PluginMetadata,
    PluginRuleDefinition, Result, TopTabConfig,
};
use serde::{Deserialize, Serialize};

/// Maximum serialized size of one executor request or response.
///
/// This is the same ceiling applied to guest export results. Keeping the
/// protocol within that bound makes the current in-process adapter suitable
/// for a future IPC transport without changing its value model.
pub const MAX_EXECUTOR_MESSAGE_BYTES: usize = 1024 * 1024;

/// One product-neutral request to an instantiated Wirt plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorRequest {
    Init,
    Metadata,
    DefaultRules,
    UiLayout {
        extension_point: PluginExtensionPoint,
    },
    UiEvent {
        id: String,
        value: Option<String>,
    },
    TopTabs,
    Cleanup,
}

impl ExecutorRequest {
    /// Reject a request that cannot cross the bounded executor protocol.
    pub fn validate(&self) -> Result<()> {
        validate_serialized_result(self).map_err(|error| message_error("request", error))
    }
}

/// One product-neutral response from an instantiated Wirt plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ExecutorResponse {
    Empty,
    Metadata(PluginMetadata),
    Rules(Vec<PluginRuleDefinition>),
    Layout(PluginLayout),
    Actions(Vec<PluginAction>),
    TopTabs(Vec<TopTabConfig>),
}

impl ExecutorResponse {
    /// Reject a response that cannot cross the bounded executor protocol.
    pub fn validate(&self) -> Result<()> {
        let result = match self {
            Self::Empty => validate_serialized_result(self),
            Self::Metadata(metadata) => validate_serialized_result(metadata),
            Self::Rules(rules) => validate_serialized_result(rules),
            Self::Layout(layout) => validate_layout_result(layout),
            Self::Actions(actions) => validate_actions_result(actions),
            Self::TopTabs(tabs) => validate_top_tabs_result(tabs),
        };
        result.map_err(|error| message_error("response", error))?;
        validate_serialized_result(self).map_err(|error| message_error("response", error))
    }

    /// Accept an empty acknowledgement or report a protocol mismatch.
    pub fn into_empty(self) -> Result<()> {
        match self {
            Self::Empty => Ok(()),
            _ => Err(unexpected_response("empty")),
        }
    }

    /// Extract metadata or report a protocol response mismatch.
    pub fn into_metadata(self) -> Result<PluginMetadata> {
        match self {
            Self::Metadata(metadata) => Ok(metadata),
            _ => Err(unexpected_response("metadata")),
        }
    }

    /// Extract neutral rules or report a protocol response mismatch.
    pub fn into_rules(self) -> Result<Vec<PluginRuleDefinition>> {
        match self {
            Self::Rules(rules) => Ok(rules),
            _ => Err(unexpected_response("rules")),
        }
    }

    /// Extract a layout or report a protocol response mismatch.
    pub fn into_layout(self) -> Result<PluginLayout> {
        match self {
            Self::Layout(layout) => Ok(layout),
            _ => Err(unexpected_response("layout")),
        }
    }

    /// Extract actions or report a protocol response mismatch.
    pub fn into_actions(self) -> Result<Vec<PluginAction>> {
        match self {
            Self::Actions(actions) => Ok(actions),
            _ => Err(unexpected_response("actions")),
        }
    }

    /// Extract top tabs or report a protocol response mismatch.
    pub fn into_top_tabs(self) -> Result<Vec<TopTabConfig>> {
        match self {
            Self::TopTabs(tabs) => Ok(tabs),
            _ => Err(unexpected_response("top tabs")),
        }
    }
}

fn unexpected_response(expected: &str) -> PluginError {
    PluginError::ExecutionError(format!(
        "executor returned an unexpected response for {expected}"
    ))
}

fn message_error(direction: &str, error: ResultValidationError) -> PluginError {
    match error {
        ResultValidationError::Quota(_) => {
            PluginError::ExecutionError(format!("executor {direction} limit exceeded"))
        }
        ResultValidationError::Serialization(error) => PluginError::Serialization(error),
    }
}

/// A request whose complete serialized envelope has passed validation.
///
/// Only Wirt can construct this wrapper. Executor backends can consume it,
/// but downstream callers cannot forge one to bypass [`WirtExecutor::execute`].
#[doc(hidden)]
pub struct ValidatedExecutorRequest(ExecutorRequest);

impl ValidatedExecutorRequest {
    #[doc(hidden)]
    pub fn into_inner(self) -> ExecutorRequest {
        self.0
    }
}

/// Host-side implementation seam for a Wirt executor.
///
/// The validated request wrapper makes this method safe to expose to adapter
/// crates without exposing a raw-message bypass.
#[doc(hidden)]
pub trait WirtExecutorBackend: Send + Sync {
    fn execute_validated(
        &self,
        plugin_id: &PluginId,
        request: ValidatedExecutorRequest,
    ) -> Result<ExecutorResponse>;
}

mod private {
    pub trait Sealed {}

    impl<T: super::WirtExecutorBackend + ?Sized> Sealed for T {}
}

/// Executes bounded Wirt messages for a plugin identity.
///
/// [`execute`](Self::execute) rejects an oversized request before registry
/// lookup or guest entry and validates the response before it crosses the
/// boundary. A blanket implementation prevents backends from overriding that
/// sequence.
///
/// The unvalidated backend entry point is not part of the public executor
/// surface:
///
/// ```compile_fail
/// # use wirt::{ExecutorRequest, PluginId, WirtExecutor};
/// # fn bypass(executor: &dyn WirtExecutor, plugin_id: &PluginId) {
/// let _ = executor.execute_unvalidated(plugin_id, ExecutorRequest::Metadata);
/// # }
/// ```
///
/// Downstream crates also cannot replace the validated implementation:
///
/// ```compile_fail
/// # use wirt::{ExecutorRequest, ExecutorResponse, PluginId, Result, WirtExecutor};
/// struct UncheckedExecutor;
/// impl WirtExecutor for UncheckedExecutor {
///     fn execute(
///         &self,
///         _plugin_id: &PluginId,
///         _request: ExecutorRequest,
///     ) -> Result<ExecutorResponse> {
///         Ok(ExecutorResponse::Empty)
///     }
/// }
/// ```
pub trait WirtExecutor: private::Sealed + Send + Sync {
    fn execute(&self, plugin_id: &PluginId, request: ExecutorRequest) -> Result<ExecutorResponse>;
}

impl<T: WirtExecutorBackend + ?Sized> WirtExecutor for T {
    fn execute(&self, plugin_id: &PluginId, request: ExecutorRequest) -> Result<ExecutorResponse> {
        request.validate()?;
        let response = self.execute_validated(plugin_id, ValidatedExecutorRequest(request))?;
        response.validate()?;
        Ok(response)
    }
}
