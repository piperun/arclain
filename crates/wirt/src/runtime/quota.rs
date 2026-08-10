use super::epoch::EPOCH_TICKS_PER_EXPORT;
use super::WirtStoreState;
use crate::limits::{StoreQuotaExceeded, StoreQuotaKind};
use crate::{
    PluginAction, PluginError, PluginLayout, PluginUiElement, Result, TopTabConfig,
    MAX_EXECUTOR_MESSAGE_BYTES,
};
use serde::Serialize;
use wasmtime::{Engine, Store};

pub(super) const FUEL_PER_EXPORT: u64 = 10_000_000;
pub(super) const MAX_UI_ELEMENTS: usize = 10_000;
pub(super) const MAX_ACTIONS: usize = 1_024;
pub(super) const MAX_SERIALIZED_RESULT_BYTES: usize = MAX_EXECUTOR_MESSAGE_BYTES;
// Measured from the largest host-side representations permitted by the current
// WIT result shapes: layout 2,568,576 bytes, top tabs 3,256,216 bytes, rules
// 2,535,880 bytes, and actions 1,155,072 bytes. Eight MiB leaves more than 2x
// headroom without retaining Wasmtime's unsafe 128 MiB default.
pub(super) const HOSTCALL_FUEL_BYTES: usize = 8 * 1024 * 1024;

// Wasmtime 47.0.1 keeps the hostcall-fuel and ResourceLimiter count error
// types private. These locked-version root-cause strings must remain exact so
// unrelated guest errors are never classified as quota failures or exposed as
// engine details.
const WASMTIME_47_HOSTCALL_FUEL_EXHAUSTED: &str =
    "too much data is being copied between the host and the guest: fuel allocated for hostcalls has been exhausted";
const WASMTIME_47_INSTANCE_COUNT_EXCEEDED: &str =
    "resource limit exceeded: instance count too high at 33";
const WASMTIME_47_MEMORY_COUNT_EXCEEDED: &str =
    "resource limit exceeded: memory count too high at 5";
const WASMTIME_47_TABLE_COUNT_EXCEEDED: &str = "resource limit exceeded: table count too high at 9";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuotaViolation {
    Result,
}

impl QuotaViolation {
    fn redacted_reason(self) -> &'static str {
        match self {
            Self::Result => "plugin result quota exceeded",
        }
    }
}

#[derive(Debug)]
pub(crate) enum ResultValidationError {
    Quota(QuotaViolation),
    Serialization(serde_json::Error),
}

#[derive(Default)]
struct ResultSizeWriter {
    bytes_written: usize,
    exceeded: bool,
}

impl std::io::Write for ResultSizeWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.bytes_written.saturating_add(buffer.len()) > MAX_SERIALIZED_RESULT_BYTES {
            self.exceeded = true;
            return Err(std::io::Error::other("plugin result quota exceeded"));
        }
        self.bytes_written += buffer.len();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn validate_serialized_result<T: Serialize + ?Sized>(
    value: &T,
) -> std::result::Result<(), ResultValidationError> {
    let mut writer = ResultSizeWriter::default();
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        return if writer.exceeded {
            Err(ResultValidationError::Quota(QuotaViolation::Result))
        } else {
            Err(ResultValidationError::Serialization(error))
        };
    }
    Ok(())
}

pub(crate) fn validate_layout_result(
    layout: &PluginLayout,
) -> std::result::Result<(), ResultValidationError> {
    fn charge_work(
        total: &mut usize,
        additional: usize,
        limit: usize,
    ) -> std::result::Result<(), ResultValidationError> {
        *total = total
            .checked_add(additional)
            .ok_or(ResultValidationError::Quota(QuotaViolation::Result))?;
        if *total > limit {
            return Err(ResultValidationError::Quota(QuotaViolation::Result));
        }
        Ok(())
    }

    fn charge_elements(
        roots: &[PluginUiElement],
        work: &mut usize,
    ) -> std::result::Result<(), ResultValidationError> {
        let mut stack = vec![roots.iter()];
        while let Some(element) = stack.last_mut().and_then(Iterator::next) {
            charge_work(work, 1, MAX_UI_ELEMENTS)?;
            match element {
                PluginUiElement::RadioGroup { options, .. }
                | PluginUiElement::Dropdown { options, .. } => {
                    charge_work(work, options.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::Tabs { tabs, .. } => {
                    charge_work(work, tabs.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::ListContainer { items, .. } => stack.push(items.iter()),
                PluginUiElement::TagChips { tags, max_display } => {
                    let display_limit = max_display
                        .map(|limit| usize::try_from(limit).unwrap_or(usize::MAX))
                        .unwrap_or(tags.len());
                    let visible = display_limit.min(tags.len());
                    let overflow_label = usize::from(visible < tags.len());
                    charge_work(work, visible, MAX_UI_ELEMENTS)?;
                    charge_work(work, overflow_label, MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::Toolbar { buttons } => {
                    charge_work(work, buttons.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::Carousel { images, .. } => {
                    charge_work(work, images.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::KeyValueList { items, .. }
                | PluginUiElement::MetadataGrid { items, .. } => {
                    charge_work(work, items.len(), MAX_UI_ELEMENTS)?;
                }
                _ => {}
            }

            while stack.last().is_some_and(|elements| elements.len() == 0) {
                stack.pop();
            }
        }
        Ok(())
    }

    let mut work = 0usize;
    match layout {
        PluginLayout::Single { elements } => charge_elements(elements, &mut work)?,
        PluginLayout::Split {
            sidebar, content, ..
        } => {
            charge_elements(sidebar, &mut work)?;
            charge_elements(content, &mut work)?;
        }
    }

    validate_serialized_result(layout)
}

pub(crate) fn validate_actions_result(
    actions: &[PluginAction],
) -> std::result::Result<(), ResultValidationError> {
    let mut work = 0usize;
    for action in actions {
        work = work
            .checked_add(1)
            .ok_or(ResultValidationError::Quota(QuotaViolation::Result))?;
        if let PluginAction::OpenLightbox { images, .. } = action {
            work = work
                .checked_add(images.len())
                .ok_or(ResultValidationError::Quota(QuotaViolation::Result))?;
        }
        if work > MAX_ACTIONS {
            return Err(ResultValidationError::Quota(QuotaViolation::Result));
        }
    }
    validate_serialized_result(actions)
}

pub(crate) fn validate_top_tabs_result(
    tabs: &[TopTabConfig],
) -> std::result::Result<(), ResultValidationError> {
    if tabs.len() > MAX_UI_ELEMENTS {
        return Err(ResultValidationError::Quota(QuotaViolation::Result));
    }
    validate_serialized_result(tabs)
}

#[derive(Debug, Default)]
pub(super) struct InstanceAvailability {
    reason: Option<&'static str>,
}

impl InstanceAvailability {
    pub(super) fn ensure_available(&self) -> Result<()> {
        match self.reason {
            Some(reason) => Err(PluginError::Unavailable(reason.to_string())),
            None => Ok(()),
        }
    }

    fn mark_unavailable<T>(&mut self, reason: &'static str) -> Result<T> {
        self.reason = Some(reason);
        Err(PluginError::Unavailable(reason.to_string()))
    }

    pub(super) fn reason(&self) -> Option<&'static str> {
        self.reason
    }
}

/// Return the redacted terminal reason for a Wasmtime failure.
///
/// Every trap is terminal: after a component trap, entering the same instance
/// again produces an internal Wasmtime error rather than a fresh guest call.
/// Ordinary guest-owned errors stay nonterminal and are mapped by the caller.
pub(super) fn resource_quota_reason(error: &wasmtime::Error) -> Option<&'static str> {
    if let Some(trap) = error.downcast_ref::<wasmtime::Trap>() {
        return match trap {
            wasmtime::Trap::OutOfFuel => Some("plugin fuel quota exceeded"),
            wasmtime::Trap::Interrupt => Some("plugin execution deadline exceeded"),
            _ => Some("plugin execution trapped"),
        };
    }

    match error.root_cause().to_string().as_str() {
        WASMTIME_47_HOSTCALL_FUEL_EXHAUSTED => {
            return Some("plugin hostcall data quota exceeded");
        }
        WASMTIME_47_INSTANCE_COUNT_EXCEEDED => {
            return Some("plugin instance quota exceeded");
        }
        WASMTIME_47_MEMORY_COUNT_EXCEEDED => {
            return Some("plugin memory quota exceeded");
        }
        WASMTIME_47_TABLE_COUNT_EXCEEDED => {
            return Some("plugin table quota exceeded");
        }
        _ => {}
    }

    error
        .downcast_ref::<StoreQuotaExceeded>()
        .map(|quota| match quota.kind {
            StoreQuotaKind::Memory => "plugin memory quota exceeded",
            StoreQuotaKind::Table => "plugin table quota exceeded",
        })
}

pub(super) fn call_with_quotas<H: WirtStoreState, T>(
    store: &mut Store<H>,
    availability: &mut InstanceAvailability,
    call: impl FnOnce(&mut Store<H>) -> wasmtime::Result<T>,
    validate: impl FnOnce(&T) -> std::result::Result<(), ResultValidationError>,
    map_ordinary_error: impl FnOnce(String) -> PluginError,
) -> Result<T> {
    availability.ensure_available()?;
    if prepare_export_quota(store).is_err() {
        return availability.mark_unavailable("plugin execution quota unavailable");
    }

    let value = match call(store) {
        Ok(value) => value,
        Err(error) => {
            if let Some(reason) = resource_quota_reason(&error) {
                return availability.mark_unavailable(reason);
            }
            return Err(map_ordinary_error(error.to_string()));
        }
    };

    match validate(&value) {
        Ok(()) => {}
        Err(ResultValidationError::Quota(violation)) => {
            return availability.mark_unavailable(violation.redacted_reason());
        }
        Err(ResultValidationError::Serialization(error)) => {
            return Err(PluginError::Serialization(error));
        }
    }
    Ok(value)
}

pub(super) fn prepare_export_quota<H>(store: &mut Store<H>) -> wasmtime::Result<()> {
    store.set_fuel(FUEL_PER_EXPORT)?;
    store.set_epoch_deadline(EPOCH_TICKS_PER_EXPORT);
    Ok(())
}

pub(super) fn new_plugin_store<H: WirtStoreState>(engine: &Engine, host: H) -> Result<Store<H>> {
    let mut store = Store::new(engine, host);
    store.set_hostcall_fuel(HOSTCALL_FUEL_BYTES);
    store.limiter(|state| state.store_limiter());
    prepare_export_quota(&mut store)
        .map_err(|_| PluginError::WasmError("failed to configure plugin fuel quota".to_string()))?;
    store.epoch_deadline_trap();
    Ok(store)
}
