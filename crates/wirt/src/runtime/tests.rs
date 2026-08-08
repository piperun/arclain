use super::epoch::{EPOCH_TICKS_PER_EXPORT, EPOCH_TICK_INTERVAL};
use super::quota::*;
use super::*;
use crate::{PluginError, Result};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::Linker;
use wasmtime::Store;

use crate as wirt_crate;
#[path = "../../tests/support/stub_host.rs"]
mod stub_host;
use stub_host::StubHost;

#[cfg(any(windows, target_os = "linux"))]
const LARGE_LIFT_BYTES: usize = 120 * 1024 * 1024;
#[cfg(any(windows, target_os = "linux"))]
const HOSTCALL_MEMORY_CHILD: &str = "WIRT_HOSTCALL_MEMORY_CHILD";
type ByteListLift = wasmtime::component::TypedFunc<(), (Vec<u8>,)>;

fn instantiate_byte_list_fixture(
    runtime: &WasmRuntime,
    byte_len: usize,
) -> anyhow::Result<(Store<StubHost>, ByteListLift)> {
    let memory_pages = (byte_len + 8).div_ceil(64 * 1024);
    let length_bytes = (byte_len as u32).to_le_bytes();
    let encoded_length = length_bytes
        .iter()
        .map(|byte| format!("\\{byte:02x}"))
        .collect::<String>();
    let component = Component::new(
        &runtime.engine,
        format!(
            r#"
                (component
                    (core module $fixture
                        (memory (export "memory") {memory_pages})
                        (data (i32.const 0) "\08\00\00\00{encoded_length}")
                        (func (export "lift") (result i32)
                            i32.const 0))
                    (core instance $instance (instantiate $fixture))
                    (alias core export $instance "memory" (core memory $memory))
                    (alias core export $instance "lift" (core func $lift))
                    (type $bytes (list u8))
                    (type $lift-type (func (result $bytes)))
                    (func (export "lift") (type $lift-type)
                        (canon lift (core func $lift) (memory $memory))))
            "#,
        ),
    )?;
    let host = StubHost::new();
    let mut store = new_plugin_store(&runtime.engine, host)?;
    let instance = Linker::new(&runtime.engine).instantiate(&mut store, &component)?;
    let lift = instance.get_typed_func::<(), (Vec<u8>,)>(&mut store, "lift")?;
    Ok((store, lift))
}

#[cfg(windows)]
fn current_working_set_bytes() -> usize {
    use std::ffi::c_void;

    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
    }

    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    // SAFETY: both functions are called with the current process pseudo-handle
    // and a correctly sized writable PROCESS_MEMORY_COUNTERS buffer.
    let succeeded = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<ProcessMemoryCounters>() as u32,
        )
    };
    assert_ne!(succeeded, 0, "GetProcessMemoryInfo must succeed");
    counters.working_set_size
}

#[cfg(target_os = "linux")]
fn current_working_set_bytes() -> usize {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let rss_kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<usize>().ok())
        .expect("/proc/self/status must contain VmRSS");
    rss_kib * 1024
}

fn instantiate_resource_fixture(runtime: &WasmRuntime, core_module: &str) -> wasmtime::Result<()> {
    let component = Component::new(&runtime.engine, resource_fixture_wat(core_module))?;
    let host = StubHost::new();
    let mut store =
        new_plugin_store(&runtime.engine, host).expect("test store configuration must succeed");
    let linker = Linker::new(&runtime.engine);
    linker.instantiate(&mut store, &component)?;
    Ok(())
}

fn instantiate_core_instance_fixture(runtime: &WasmRuntime, count: usize) -> wasmtime::Result<()> {
    let component = Component::new(&runtime.engine, core_instance_fixture_wat(count))?;
    let host = StubHost::new();
    let mut store =
        new_plugin_store(&runtime.engine, host).expect("test store configuration must succeed");
    Linker::new(&runtime.engine).instantiate(&mut store, &component)?;
    Ok(())
}

fn resource_fixture_wat(core_module: &str) -> String {
    format!(
        "(component (core module $fixture {core_module}) (core instance $instance (instantiate $fixture)))"
    )
}

fn core_instance_fixture_wat(count: usize) -> String {
    let instances = "(core instance (instantiate $fixture))".repeat(count);
    format!("(component (core module $fixture) {instances})")
}

fn push_u32_leb(bytes: &mut Vec<u8>, mut value: usize) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        bytes.push(byte);
        if value == 0 {
            return;
        }
    }
}

fn read_u32_leb(bytes: &[u8], cursor: &mut usize) -> usize {
    let mut value = 0usize;
    let mut shift = 0usize;
    loop {
        let byte = bytes[*cursor];
        *cursor += 1;
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

fn plugin_with_extra_core_instances(
    base: &[u8],
    core_module: &[u8],
    instance_count: usize,
) -> Vec<u8> {
    const COMPONENT_HEADER_BYTES: usize = 8;
    const CORE_MODULE_SECTION: u8 = 1;
    const CORE_INSTANCE_SECTION: u8 = 2;

    let mut cursor = COMPONENT_HEADER_BYTES;
    let mut module_index = 0usize;
    while cursor < base.len() {
        let section_id = base[cursor];
        cursor += 1;
        let section_len = read_u32_leb(base, &mut cursor);
        if section_id == CORE_MODULE_SECTION {
            module_index += 1;
        }
        cursor += section_len;
    }
    assert_eq!(cursor, base.len());

    let mut bytes = base.to_vec();
    bytes.push(CORE_MODULE_SECTION);
    push_u32_leb(&mut bytes, core_module.len());
    bytes.extend_from_slice(core_module);

    let mut instances = Vec::new();
    push_u32_leb(&mut instances, instance_count);
    for _ in 0..instance_count {
        instances.push(0x00);
        push_u32_leb(&mut instances, module_index);
        instances.push(0x00);
    }
    bytes.push(CORE_INSTANCE_SECTION);
    push_u32_leb(&mut bytes, instances.len());
    bytes.extend(instances);
    bytes
}

fn loaded_binary_fixture(runtime: &WasmRuntime, id: &str, bytes: &[u8]) -> LoadedComponent {
    LoadedComponent {
        id: id.to_string(),
        component: Component::from_binary(&runtime.engine, bytes).unwrap(),
        engine: runtime.engine.clone(),
        epoch_ticker: runtime.epoch_ticker.clone(),
        _path: std::path::PathBuf::from("<resource-limit-test>"),
    }
}

fn assert_unavailable_reason(result: Result<PluginInstance<StubHost>>, expected: &str) {
    match result {
        Err(PluginError::Unavailable(reason)) => assert_eq!(reason, expected),
        Err(error) => panic!("expected redacted unavailable error, got {error}"),
        Ok(_) => panic!("resource-limit fixture unexpectedly instantiated"),
    }
}

#[test]
fn runtime_engine_enables_fuel_metering() {
    let runtime = WasmRuntime::new().unwrap();
    let mut store = Store::new(&runtime.engine, ());

    store
        .set_fuel(1)
        .expect("every runtime store must support fuel metering");
}

// A component whose export makes exactly one host call and then returns
// through a short loop (the loop backedge is where wasmtime inserts an
// epoch check, so a deadline that expired during the hostcall is
// observed on return). The hostcall itself does nothing but take
// wall-clock time -- the shape of a network/disk hostcall, or of a
// worker thread descheduled on a loaded machine.
const SLOW_HOSTCALL_COMPONENT_WAT: &str = r#"
    (component
        (import "block" (func $host-block))
        (core module $m
            (import "host" "block" (func $block))
            (func (export "run")
                (local $i i32)
                call $block
                (local.set $i (i32.const 8))
                (loop $spin
                    (local.set $i (i32.sub (local.get $i) (i32.const 1)))
                    (br_if $spin (i32.ne (local.get $i) (i32.const 0)))
                )
            )
        )
        (core func $block-lowered (canon lower (func $host-block)))
        (core instance $host-inst
            (export "block" (func $block-lowered))
        )
        (core instance $inst (instantiate $m (with "host" (instance $host-inst))))
        (func (export "run") (canon lift (core func $inst "run")))
    )
"#;

// The wedge shape the epoch deadline exists for: an unbounded loop over
// a cheap hostcall. Each iteration burns a handful of fuel but an
// arbitrary amount of wall-clock, so fuel alone would let this pin the
// plugin worker for minutes.
const WEDGED_HOSTCALL_LOOP_WAT: &str = r#"
    (component
        (import "block" (func $host-block))
        (core module $m
            (import "host" "block" (func $block))
            (func (export "run")
                (loop $forever
                    call $block
                    br $forever
                )
            )
        )
        (core func $block-lowered (canon lower (func $host-block)))
        (core instance $host-inst
            (export "block" (func $block-lowered))
        )
        (core instance $inst (instantiate $m (with "host" (instance $host-inst))))
        (func (export "run") (canon lift (core func $inst "run")))
    )
"#;

fn instantiate_hostcall_component(
    runtime: &WasmRuntime,
    wat: &str,
    hostcall_duration: Duration,
) -> (Store<StubHost>, wasmtime::component::TypedFunc<(), ()>) {
    let component = Component::new(&runtime.engine, wat).unwrap();
    let host = StubHost::new();
    let mut store = new_plugin_store(&runtime.engine, host).unwrap();
    let mut linker: Linker<StubHost> = Linker::new(&runtime.engine);
    linker
        .root()
        .func_wrap("block", move |_store, (): ()| {
            std::thread::sleep(hostcall_duration);
            Ok(())
        })
        .unwrap();
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let run = instance
        .get_typed_func::<(), ()>(&mut store, "run")
        .unwrap();
    (store, run)
}

/// The property the epoch deadline broke when it was a ~100ms budget: a
/// guest whose export spends wall-clock in a single legitimate hostcall
/// (150ms here -- above the old ten-tick ceiling) while consuming almost
/// no fuel must complete, twice in a row, with the instance staying
/// available. Against `EPOCH_TICKS_PER_EXPORT = 10` this fails with
/// `Unavailable("plugin execution deadline exceeded")` on the first call.
#[test]
fn legitimate_slow_hostcall_work_is_not_trapped_by_the_epoch_deadline() {
    let runtime = WasmRuntime::new().unwrap();
    let (mut store, run) = instantiate_hostcall_component(
        &runtime,
        SLOW_HOSTCALL_COMPONENT_WAT,
        Duration::from_millis(150),
    );
    let mut availability = InstanceAvailability::default();

    for call in 0..2 {
        call_with_quotas(
            &mut store,
            &mut availability,
            |store| run.call(store, ()),
            |_| Ok(()),
            PluginError::ExecutionError,
        )
        .unwrap_or_else(|error| {
            panic!("well-behaved slow-hostcall export was rejected on call {call}: {error}")
        });
    }
    let fuel_used = FUEL_PER_EXPORT - store.get_fuel().unwrap();
    assert!(
        fuel_used < 1_000,
        "the slow export must be cheap in fuel (used {fuel_used}) -- wall-clock was its \
         only cost, which is precisely what the deadline must not punish"
    );
    assert_eq!(availability.reason(), None);
}

/// The wedge the dead-man switch exists for still dies: an unbounded
/// hostcall loop burns wall-clock without meaningful fuel, and the
/// free-running ticker + deadline trap + terminal classification must
/// reap it. Armed at a test-scale deadline (a few ticks) after
/// instantiation, because waiting out the real minutes-scale production
/// ceiling is not a unit test; what this pins is the mechanism the
/// production constant relies on, and the sizing test below pins the
/// constant itself.
#[test]
fn a_wedged_hostcall_loop_is_still_trapped_by_the_epoch_deadline() {
    let runtime = WasmRuntime::new().unwrap();
    let (mut store, run) = instantiate_hostcall_component(
        &runtime,
        WEDGED_HOSTCALL_LOOP_WAT,
        Duration::from_millis(5),
    );

    store.set_fuel(FUEL_PER_EXPORT).unwrap();
    store.set_epoch_deadline(5);
    let error = run.call(&mut store, ()).unwrap_err();

    assert_eq!(
        resource_quota_reason(&error),
        Some("plugin execution deadline exceeded"),
        "a hostcall loop that outlives the epoch deadline must trap terminally"
    );
    let fuel_used = FUEL_PER_EXPORT - store.get_fuel().unwrap_or(0);
    assert!(
        fuel_used < FUEL_PER_EXPORT / 100,
        "the wedge burned only {fuel_used} fuel -- fuel alone could not have reaped it"
    );
}

/// Pins the floor the dead-man switch may never shrink below: four
/// network-request timeouts, the bottom of the minutes band. The
/// binding constraint is the doctrine on `EPOCH_TICKS_PER_EXPORT`
/// itself -- the ceiling must dwarf the slowest legitimate export,
/// which is a few sequential hostcalls each bounded by the network
/// layer's 30s per-request contract -- and this assertion is the
/// arithmetic floor beneath that doctrine, so a re-tune back toward a
/// per-export "budget" fails a test instead of only contradicting a
/// comment. The shipped value sits at ten request-timeouts,
/// comfortably above the floor; anything inside the minutes band
/// satisfies both.
#[test]
fn the_epoch_deadline_dwarfs_the_slowest_legitimate_hostcall() {
    const SLOWEST_HOSTCALL_TIMEOUT: Duration = Duration::from_secs(30);
    let ceiling = EPOCH_TICK_INTERVAL * u32::try_from(EPOCH_TICKS_PER_EXPORT).unwrap();
    assert!(
        ceiling >= 4 * SLOWEST_HOSTCALL_TIMEOUT,
        "epoch ceiling {ceiling:?} must stay at or above four times the network \
         layer's per-request timeout ({:?}) -- it is a liveness backstop sized for \
         the slowest legitimate export, not a work budget",
        SLOWEST_HOSTCALL_TIMEOUT
    );
}

// Working-set telemetry is deliberately OS-specific; functional boundary
// coverage for hostcall fuel remains platform-independent below.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn hostcall_fuel_prevents_large_prevalidation_lift_allocation() {
    if std::env::var_os(HOSTCALL_MEMORY_CHILD).is_some() {
        let runtime = WasmRuntime::new().unwrap();
        let (mut store, lift) = instantiate_byte_list_fixture(&runtime, LARGE_LIFT_BYTES).unwrap();
        let before = current_working_set_bytes();
        let lifted = lift.call(&mut store, ()).ok();
        std::hint::black_box(&lifted);
        let delta = current_working_set_bytes().saturating_sub(before);
        println!(
            "WIRT_HOSTCALL_MEASUREMENT lifted={} rss_delta={delta}",
            lifted.is_some()
        );
        return;
    }

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime::tests::hostcall_fuel_prevents_large_prevalidation_lift_allocation",
            "--nocapture",
        ])
        .env(HOSTCALL_MEMORY_CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let measurement = stdout
        .lines()
        .find(|line| line.contains("WIRT_HOSTCALL_MEASUREMENT"))
        .unwrap_or_else(|| panic!("child did not report memory measurement:\n{stdout}"));
    let lifted = measurement.contains("lifted=true");
    let delta = measurement
        .split("rss_delta=")
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap();

    assert!(
        !lifted && delta < 32 * 1024 * 1024,
        "a 120 MiB guest result was lifted before validation: {measurement}"
    );
}

#[test]
fn configured_hostcall_fuel_covers_measured_current_wit_shapes() {
    use crate::bindings::wirt::plugin::rules::PluginRuleDefinition as WitRule;
    use crate::bindings::wirt::plugin::ui::{
        KeyValuePair as WitKeyValuePair, ListItemConfig as WitListItem, PluginAction as WitAction,
        ToolbarButtonConfig as WitToolbarButton, TopTabConfig as WitTopTab,
        UiElement as WitUiElement,
    };

    const DOCUMENTED_HOSTCALL_FUEL_BYTES: usize = 8 * 1024 * 1024;
    assert!(
        crate::MAX_PLUGIN_GUEST_DATA_BYTES <= HOSTCALL_FUEL_BYTES / 2,
        "guest-return body cap must leave at least half of actual hostcall fuel for WIT lifting overhead"
    );

    fn max_serialized_list_len<T: Serialize>(minimum_item: &T) -> usize {
        let item_bytes = serde_json::to_vec(minimum_item).unwrap().len();
        (MAX_SERIALIZED_RESULT_BYTES - 2) / (item_bytes + 1) + 1
    }

    let largest_ui_allocation = [
        std::mem::size_of::<WitUiElement>(),
        std::mem::size_of::<WitListItem>(),
        std::mem::size_of::<WitToolbarButton>(),
        std::mem::size_of::<WitKeyValuePair>(),
        std::mem::size_of::<(String, Option<String>)>(),
        std::mem::size_of::<String>(),
    ]
    .into_iter()
    .max()
    .unwrap();
    let layout_bytes =
        MAX_SERIALIZED_RESULT_BYTES + MAX_UI_ELEMENTS.saturating_mul(largest_ui_allocation);

    let action_bytes = MAX_SERIALIZED_RESULT_BYTES
        + MAX_ACTIONS.saturating_mul(
            std::mem::size_of::<WitAction>() + std::mem::size_of::<(String, Option<String>)>(),
        );

    let minimum_tab = crate::TopTabConfig {
        id: String::new(),
        label: String::new(),
        icon: String::new(),
        badge: None,
        priority: 0,
    };
    let top_tab_bytes = MAX_SERIALIZED_RESULT_BYTES
        + max_serialized_list_len(&minimum_tab).saturating_mul(std::mem::size_of::<WitTopTab>());

    let minimum_rule = crate::PluginRuleDefinition {
        name: String::new(),
        category: String::new(),
        description: None,
        trigger: crate::PluginRuleTrigger {
            filename_pattern: None,
            has_file: None,
            extensions: None,
            min_size: None,
            max_size: None,
            metadata_source: None,
        },
        actions: crate::PluginRuleActions {
            root_folder: None,
            move_files: Vec::new(),
            move_to: None,
            rename_pattern: None,
            organize_content: false,
            delete_original: false,
            use_standard_layout: false,
        },
    };
    let rule_bytes = MAX_SERIALIZED_RESULT_BYTES
        + max_serialized_list_len(&minimum_rule).saturating_mul(std::mem::size_of::<WitRule>());

    let worst_shape_bytes = [layout_bytes, action_bytes, top_tab_bytes, rule_bytes]
        .into_iter()
        .max()
        .unwrap();
    assert!(
        worst_shape_bytes <= DOCUMENTED_HOSTCALL_FUEL_BYTES,
        "current WIT host shape needs {worst_shape_bytes} bytes of hostcall fuel"
    );

    let runtime = WasmRuntime::new().unwrap();
    let host = StubHost::new();
    let store = new_plugin_store(&runtime.engine, host).unwrap();
    assert_eq!(
        store.hostcall_fuel(),
        DOCUMENTED_HOSTCALL_FUEL_BYTES,
        "production stores must use the measured current-WIT hostcall budget; estimates: layout={layout_bytes}, actions={action_bytes}, tabs={top_tab_bytes}, rules={rule_bytes}"
    );
}

#[test]
fn hostcall_fuel_accepts_exact_boundary_and_terminally_rejects_one_over() {
    let runtime = WasmRuntime::new().unwrap();
    let (mut exact_store, exact_lift) =
        instantiate_byte_list_fixture(&runtime, HOSTCALL_FUEL_BYTES).unwrap();
    let exact = exact_lift.call(&mut exact_store, ()).unwrap().0;
    assert_eq!(exact.len(), HOSTCALL_FUEL_BYTES);

    let (mut over_store, over_lift) =
        instantiate_byte_list_fixture(&runtime, HOSTCALL_FUEL_BYTES + 1).unwrap();
    let mut availability = InstanceAvailability::default();
    let call_entries = std::sync::atomic::AtomicUsize::new(0);
    let first = call_with_quotas(
        &mut over_store,
        &mut availability,
        |store| {
            call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            over_lift.call(store, ()).map(|_| ())
        },
        |_| Ok(()),
        PluginError::ExecutionError,
    )
    .unwrap_err();
    let second = call_with_quotas(
        &mut over_store,
        &mut availability,
        |store| {
            call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            over_lift.call(store, ()).map(|_| ())
        },
        |_| Ok(()),
        PluginError::ExecutionError,
    )
    .unwrap_err();

    assert!(matches!(
        first,
        PluginError::Unavailable(ref reason)
            if reason == "plugin hostcall data quota exceeded"
    ));
    assert!(matches!(second, PluginError::Unavailable(_)));
    assert_eq!(call_entries.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        availability.reason(),
        Some("plugin hostcall data quota exceeded")
    );
}

#[test]
fn fuel_epoch_memory_and_table_quota_errors_have_redacted_classifications() {
    let fuel = wasmtime::Error::from(wasmtime::Trap::OutOfFuel);
    let epoch = wasmtime::Error::from(wasmtime::Trap::Interrupt);
    let memory = wasmtime::Error::new(crate::StoreQuotaExceeded {
        kind: crate::StoreQuotaKind::Memory,
    });
    let table = wasmtime::Error::new(crate::StoreQuotaExceeded {
        kind: crate::StoreQuotaKind::Table,
    });

    assert_eq!(
        resource_quota_reason(&fuel),
        Some("plugin fuel quota exceeded")
    );
    assert_eq!(
        resource_quota_reason(&epoch),
        Some("plugin execution deadline exceeded")
    );
    assert_eq!(
        resource_quota_reason(&memory),
        Some("plugin memory quota exceeded")
    );
    assert_eq!(
        resource_quota_reason(&table),
        Some("plugin table quota exceeded")
    );
}

/// A guest trap that is *not* one of the two quota-shaped variants
/// (a real out-of-bounds panic, `unreachable`, an integer division
/// by zero, ...) must still be classified terminal -- see
/// `resource_quota_reason`'s own doc comment for why every trap
/// variant permanently poisons its `Store`, not just fuel/interrupt.
#[test]
fn a_generic_guest_trap_is_also_classified_as_terminal() {
    let unreachable = wasmtime::Error::from(wasmtime::Trap::UnreachableCodeReached);
    let division_by_zero = wasmtime::Error::from(wasmtime::Trap::IntegerDivisionByZero);

    assert_eq!(
        resource_quota_reason(&unreachable),
        Some("plugin execution trapped")
    );
    assert_eq!(
        resource_quota_reason(&division_by_zero),
        Some("plugin execution trapped")
    );
}

#[test]
fn serialized_result_limit_accepts_exact_boundary_and_rejects_one_byte_over() {
    let exact = "x".repeat(MAX_SERIALIZED_RESULT_BYTES - 2);
    let over = "x".repeat(MAX_SERIALIZED_RESULT_BYTES - 1);

    assert!(validate_serialized_result(&exact).is_ok());
    assert!(matches!(
        validate_serialized_result(&over),
        Err(ResultValidationError::Quota(QuotaViolation::Result))
    ));
}

#[test]
fn ui_element_limit_counts_nested_elements_at_the_boundary() {
    use crate::{PluginLayout, PluginUiElement};

    let nested_items = vec![PluginUiElement::Separator; MAX_UI_ELEMENTS - 1];
    let exact = PluginLayout::Single {
        elements: vec![PluginUiElement::ListContainer {
            id: "items".to_string(),
            items: nested_items,
            max_height: None,
            empty_message: None,
        }],
    };
    assert!(validate_layout_result(&exact).is_ok());

    let over = PluginLayout::Single {
        elements: vec![PluginUiElement::Separator; MAX_UI_ELEMENTS + 1],
    };
    assert!(matches!(
        validate_layout_result(&over),
        Err(ResultValidationError::Quota(QuotaViolation::Result))
    ));
}

#[test]
fn every_rendered_layout_collection_counts_boundary_and_one_over() {
    use crate::{KeyValuePair, PluginLayout, PluginUiElement, ToolbarButton};

    let strings = |count| vec![String::new(); count];
    let toolbar_buttons = |count| {
        vec![
            ToolbarButton {
                id: String::new(),
                label: String::new(),
                icon: None,
                primary: false,
                spacer_before: false,
            };
            count
        ]
    };
    let pairs = |count| {
        vec![
            KeyValuePair {
                key: String::new(),
                value: String::new(),
            };
            count
        ]
    };
    let images = |count| vec![(String::new(), None); count];
    let single = |element| PluginLayout::Single {
        elements: vec![element],
    };

    let cases = vec![
        (
            "radio options",
            single(PluginUiElement::RadioGroup {
                id: String::new(),
                label: String::new(),
                options: strings(MAX_UI_ELEMENTS - 1),
                selected: String::new(),
            }),
            single(PluginUiElement::RadioGroup {
                id: String::new(),
                label: String::new(),
                options: strings(MAX_UI_ELEMENTS),
                selected: String::new(),
            }),
        ),
        (
            "dropdown options",
            single(PluginUiElement::Dropdown {
                id: String::new(),
                label: String::new(),
                options: strings(MAX_UI_ELEMENTS - 1),
                selected: String::new(),
            }),
            single(PluginUiElement::Dropdown {
                id: String::new(),
                label: String::new(),
                options: strings(MAX_UI_ELEMENTS),
                selected: String::new(),
            }),
        ),
        (
            "tabs",
            single(PluginUiElement::Tabs {
                id: String::new(),
                tabs: strings(MAX_UI_ELEMENTS - 1),
                selected: String::new(),
            }),
            single(PluginUiElement::Tabs {
                id: String::new(),
                tabs: strings(MAX_UI_ELEMENTS),
                selected: String::new(),
            }),
        ),
        (
            "toolbar buttons",
            single(PluginUiElement::Toolbar {
                buttons: toolbar_buttons(MAX_UI_ELEMENTS - 1),
            }),
            single(PluginUiElement::Toolbar {
                buttons: toolbar_buttons(MAX_UI_ELEMENTS),
            }),
        ),
        (
            "carousel images",
            single(PluginUiElement::Carousel {
                id: String::new(),
                images: images(MAX_UI_ELEMENTS - 1),
                current_index: 0,
                max_height: None,
                thumbnail_height: None,
                enable_lightbox: true,
            }),
            single(PluginUiElement::Carousel {
                id: String::new(),
                images: images(MAX_UI_ELEMENTS),
                current_index: 0,
                max_height: None,
                thumbnail_height: None,
                enable_lightbox: true,
            }),
        ),
        (
            "key-value list items",
            single(PluginUiElement::KeyValueList {
                items: pairs(MAX_UI_ELEMENTS - 1),
                columns: None,
            }),
            single(PluginUiElement::KeyValueList {
                items: pairs(MAX_UI_ELEMENTS),
                columns: None,
            }),
        ),
        (
            "metadata grid items",
            single(PluginUiElement::MetadataGrid {
                items: pairs(MAX_UI_ELEMENTS - 1),
                columns: None,
            }),
            single(PluginUiElement::MetadataGrid {
                items: pairs(MAX_UI_ELEMENTS),
                columns: None,
            }),
        ),
        (
            "tag chips",
            single(PluginUiElement::TagChips {
                tags: strings(MAX_UI_ELEMENTS - 1),
                max_display: None,
            }),
            single(PluginUiElement::TagChips {
                tags: strings(MAX_UI_ELEMENTS),
                max_display: None,
            }),
        ),
    ];

    for (name, exact, over) in cases {
        assert!(
            validate_layout_result(&exact).is_ok(),
            "{name} must accept the exact rendered-work boundary"
        );
        assert!(
            matches!(
                validate_layout_result(&over),
                Err(ResultValidationError::Quota(QuotaViolation::Result))
            ),
            "{name} must reject one rendered work item over"
        );
    }
}

#[test]
fn tag_chip_work_matches_visible_tags_plus_overflow_label() {
    let layout = crate::PluginLayout::Single {
        elements: vec![crate::PluginUiElement::TagChips {
            tags: vec![String::new(); MAX_UI_ELEMENTS + 1],
            max_display: Some(0),
        }],
    };

    assert!(
        validate_layout_result(&layout).is_ok(),
        "zero visible tags render one element plus the +N-more label"
    );
}

#[test]
fn split_nested_layout_combines_all_rendered_work() {
    use crate::{PluginLayout, PluginUiElement, ToolbarButton};

    let layout = |button_count| PluginLayout::Split {
        sidebar: vec![PluginUiElement::ListContainer {
            id: String::new(),
            items: vec![PluginUiElement::Separator; 4_998],
            max_height: None,
            empty_message: None,
        }],
        content: vec![PluginUiElement::Toolbar {
            buttons: vec![
                ToolbarButton {
                    id: String::new(),
                    label: String::new(),
                    icon: None,
                    primary: false,
                    spacer_before: false,
                };
                button_count
            ],
        }],
        sidebar_width: None,
    };

    assert!(validate_layout_result(&layout(5_000)).is_ok());
    assert!(matches!(
        validate_layout_result(&layout(5_001)),
        Err(ResultValidationError::Quota(QuotaViolation::Result))
    ));
}

#[test]
fn action_limit_accepts_boundary_and_rejects_one_over() {
    let exact = vec![crate::PluginAction::None; MAX_ACTIONS];
    let over = vec![crate::PluginAction::None; MAX_ACTIONS + 1];

    assert!(validate_actions_result(&exact).is_ok());
    assert!(matches!(
        validate_actions_result(&over),
        Err(ResultValidationError::Quota(QuotaViolation::Result))
    ));
}

#[test]
fn lightbox_images_share_the_action_work_budget() {
    let action = |image_count| {
        vec![crate::PluginAction::OpenLightbox {
            images: vec![(String::new(), None); image_count],
            start_index: 0,
            title: None,
        }]
    };

    assert!(validate_actions_result(&action(MAX_ACTIONS - 1)).is_ok());
    assert!(matches!(
        validate_actions_result(&action(MAX_ACTIONS)),
        Err(ResultValidationError::Quota(QuotaViolation::Result))
    ));
}

fn minimal_top_tab() -> crate::TopTabConfig {
    crate::TopTabConfig {
        id: String::new(),
        label: String::new(),
        icon: String::new(),
        badge: None,
        priority: 0,
    }
}

#[test]
fn top_tab_limit_accepts_boundary_and_rejects_one_over() {
    let exact = vec![minimal_top_tab(); MAX_UI_ELEMENTS];
    let over = vec![minimal_top_tab(); MAX_UI_ELEMENTS + 1];

    assert!(validate_top_tabs_result(&exact).is_ok());
    assert!(matches!(
        validate_top_tabs_result(&over),
        Err(ResultValidationError::Quota(QuotaViolation::Result))
    ));
}

#[test]
fn oversized_top_tabs_are_terminal_and_second_call_skips_guest_boundary() {
    let runtime = WasmRuntime::new().unwrap();
    let host = StubHost::new();
    let mut store = new_plugin_store(&runtime.engine, host).unwrap();
    let mut availability = InstanceAvailability::default();
    let call_entries = std::sync::atomic::AtomicUsize::new(0);
    let over = vec![minimal_top_tab(); MAX_UI_ELEMENTS + 1];

    let first = call_with_quotas(
        &mut store,
        &mut availability,
        |_| {
            call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(over.clone())
        },
        |tabs| validate_top_tabs_result(tabs),
        PluginError::ExecutionError,
    )
    .unwrap_err();
    let second = call_with_quotas(
        &mut store,
        &mut availability,
        |_| {
            call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Vec::<crate::TopTabConfig>::new())
        },
        |tabs| validate_top_tabs_result(tabs),
        PluginError::ExecutionError,
    )
    .unwrap_err();

    assert!(matches!(
        first,
        PluginError::Unavailable(ref reason) if reason == "plugin result quota exceeded"
    ));
    assert!(matches!(second, PluginError::Unavailable(_)));
    assert_eq!(call_entries.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(availability.reason(), Some("plugin result quota exceeded"));
}

#[test]
fn linear_memory_limit_accepts_boundary_and_rejects_one_page_over() {
    let runtime = WasmRuntime::new().unwrap();

    instantiate_resource_fixture(&runtime, "(memory 4096)")
        .expect("256 MiB linear memory must remain available");
    let over_limit = instantiate_resource_fixture(&runtime, "(memory 4097)");
    assert!(
        over_limit.is_err(),
        "linear memory above 256 MiB must be rejected"
    );
}

#[test]
fn table_element_limit_accepts_boundary_and_rejects_one_over() {
    let runtime = WasmRuntime::new().unwrap();

    instantiate_resource_fixture(&runtime, "(table 100000 funcref)")
        .expect("100,000 table elements must remain available");
    assert!(
        instantiate_resource_fixture(&runtime, "(table 100001 funcref)").is_err(),
        "table element counts above 100,000 must be rejected"
    );
}

#[test]
fn table_count_limit_accepts_eight_and_rejects_nine() {
    let runtime = WasmRuntime::new().unwrap();
    let at_limit = "(table 1 funcref)".repeat(8);
    let over_limit = "(table 1 funcref)".repeat(9);

    instantiate_resource_fixture(&runtime, &at_limit).expect("eight tables must remain available");
    assert!(
        instantiate_resource_fixture(&runtime, &over_limit).is_err(),
        "a ninth table must be rejected"
    );
}

#[test]
fn memory_count_limit_accepts_four_and_rejects_five() {
    let runtime = WasmRuntime::new().unwrap();
    let at_limit = "(memory 1)".repeat(4);
    let over_limit = "(memory 1)".repeat(5);

    instantiate_resource_fixture(&runtime, &at_limit).expect("four memories must remain available");
    assert!(
        instantiate_resource_fixture(&runtime, &over_limit).is_err(),
        "a fifth memory must be rejected"
    );
}

#[test]
fn core_instance_limit_accepts_compatibility_boundary_and_rejects_one_over() {
    let runtime = WasmRuntime::new().unwrap();
    instantiate_core_instance_fixture(&runtime, crate::MAX_CORE_INSTANCES)
        .expect("the compatibility-safe core-instance boundary must remain available");
    assert!(
        instantiate_core_instance_fixture(&runtime, crate::MAX_CORE_INSTANCES + 1).is_err(),
        "a core instance above the compatibility boundary must be rejected"
    );
}

#[test]
fn locked_wasmtime_resource_count_causes_are_redacted_quota_errors() {
    let runtime = WasmRuntime::new().unwrap();
    let instance =
        instantiate_core_instance_fixture(&runtime, crate::MAX_CORE_INSTANCES + 1).unwrap_err();
    let memory = instantiate_resource_fixture(&runtime, &"(memory 1)".repeat(5)).unwrap_err();
    let table = instantiate_resource_fixture(&runtime, &"(table 1 funcref)".repeat(9)).unwrap_err();

    assert_eq!(
        instance.root_cause().to_string(),
        "resource limit exceeded: instance count too high at 33"
    );
    assert_eq!(
        memory.root_cause().to_string(),
        "resource limit exceeded: memory count too high at 5"
    );
    assert_eq!(
        table.root_cause().to_string(),
        "resource limit exceeded: table count too high at 9"
    );
    assert_eq!(
        resource_quota_reason(&instance),
        Some("plugin instance quota exceeded")
    );
    assert_eq!(
        resource_quota_reason(&memory),
        Some("plugin memory quota exceeded")
    );
    assert_eq!(
        resource_quota_reason(&table),
        Some("plugin table quota exceeded")
    );
}

#[test]
fn resource_count_errors_are_redacted_during_generic_plugin_instantiation() {
    let runtime = WasmRuntime::new().unwrap();
    let base = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    const EMPTY_CORE_MODULE: &[u8] = b"\0asm\x01\0\0\0";
    // ui-demo already owns one linear memory and two tables. These modules
    // take the real plugin to Wasmtime's first rejected totals (5 and 9).
    const FOUR_MEMORY_CORE_MODULE: &[u8] = b"\0asm\x01\0\0\0\x05\x09\x04\0\x01\0\x01\0\x01\0\x01";
    const SEVEN_TABLE_CORE_MODULE: &[u8] = b"\0asm\x01\0\0\0\x04\x16\x07\x70\0\x01\x70\0\x01\x70\0\x01\x70\0\x01\x70\0\x01\x70\0\x01\x70\0\x01";
    let fixtures = [
        (
            "instance-count",
            plugin_with_extra_core_instances(
                base,
                EMPTY_CORE_MODULE,
                crate::MAX_CORE_INSTANCES + 1,
            ),
            "plugin instance quota exceeded",
        ),
        (
            "memory-count",
            plugin_with_extra_core_instances(base, FOUR_MEMORY_CORE_MODULE, 1),
            "plugin memory quota exceeded",
        ),
        (
            "table-count",
            plugin_with_extra_core_instances(base, SEVEN_TABLE_CORE_MODULE, 1),
            "plugin table quota exceeded",
        ),
    ];
    for (id, bytes, expected) in fixtures {
        let loaded = loaded_binary_fixture(&runtime, id, &bytes);
        assert_unavailable_reason(loaded.instantiate(StubHost::new()), expected);
    }
}

#[test]
fn bundled_components_instantiate_with_the_compatibility_safe_core_instance_limit() {
    let runtime = WasmRuntime::new().unwrap();
    for (id, bytes) in [
        (
            "ui-demo",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../plugins/ui-demo/ui-demo.wasm"
            ))
            .as_slice(),
        ),
        (
            "malicious-metadata",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../plugins/tests/fixtures/malicious-metadata/malicious-metadata.wasm"
            ))
            .as_slice(),
        ),
    ] {
        let loaded = runtime
            .load_component_from_bytes(id.to_string(), bytes)
            .unwrap();
        let mut instance = loaded
            .instantiate(StubHost::new())
            .unwrap_or_else(|error| panic!("{id} must fit the core-instance quota: {error}"));
        instance
            .get_metadata()
            .unwrap_or_else(|error| panic!("{id} metadata must execute under quotas: {error}"));
    }
}

#[test]
fn runtime_drop_stops_and_joins_epoch_ticker() {
    let runtime = WasmRuntime::new().unwrap();
    let ticker_exited = runtime.epoch_ticker_exit_probe();

    drop(runtime);

    assert!(
        ticker_exited.load(std::sync::atomic::Ordering::Acquire),
        "runtime Drop must join the epoch ticker before returning"
    );
}

#[test]
fn infinite_component_becomes_terminal_and_second_call_never_reenters_guest() {
    let (outcome_tx, outcome_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let outcome = (|| -> anyhow::Result<(PluginError, PluginError, i32, Duration)> {
            let runtime = WasmRuntime::new()?;
            let component = Component::new(
                &runtime.engine,
                r#"
                    (component
                        (core module $fixture
                            (func (export "run")
                                (loop $spin (br $spin))))
                        (core instance $instance (instantiate $fixture))
                        (func (export "run")
                            (canon lift (core func $instance "run"))))
                "#,
            )?;
            let host = StubHost::new();
            let mut store = new_plugin_store(&runtime.engine, host)?;
            let instance = Linker::new(&runtime.engine).instantiate(&mut store, &component)?;
            let run = instance.get_typed_func::<(), ()>(&mut store, "run")?;
            let mut availability = InstanceAvailability::default();
            let call_entries = Arc::new(std::sync::atomic::AtomicUsize::new(0));

            let first_entries = call_entries.clone();
            let first = call_with_quotas(
                &mut store,
                &mut availability,
                |store| {
                    first_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    run.call(store, ())
                },
                |_| Ok(()),
                PluginError::ExecutionError,
            )
            .unwrap_err();

            let second_started = std::time::Instant::now();
            let second_entries = call_entries.clone();
            let second = call_with_quotas(
                &mut store,
                &mut availability,
                |store| {
                    second_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    run.call(store, ())
                },
                |_| Ok(()),
                PluginError::ExecutionError,
            )
            .unwrap_err();
            let second_elapsed = second_started.elapsed();
            let guest_entries = call_entries.load(std::sync::atomic::Ordering::SeqCst) as i32;
            Ok((first, second, guest_entries, second_elapsed))
        })();
        let _ = outcome_tx.send(outcome);
    });

    let (first, second, guest_entries, second_elapsed) = outcome_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("infinite guest call must terminate under a wall-clock guard")
        .unwrap();
    assert!(matches!(first, PluginError::Unavailable(_)));
    assert!(matches!(second, PluginError::Unavailable(_)));
    assert_eq!(
        guest_entries, 1,
        "the terminal call must not enter the guest"
    );
    assert!(
        second_elapsed < Duration::from_millis(10),
        "terminal calls must fail before crossing the Wasm boundary"
    );
}

#[test]
fn ordinary_guest_error_does_not_make_instance_terminal() {
    let runtime = WasmRuntime::new().unwrap();
    let host = StubHost::new();
    let mut store = new_plugin_store(&runtime.engine, host).unwrap();
    let mut availability = InstanceAvailability::default();
    let call_entries = std::sync::atomic::AtomicUsize::new(0);

    let first = call_with_quotas(
        &mut store,
        &mut availability,
        |_| -> wasmtime::Result<()> {
            call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(wasmtime::Error::msg("guest-owned secret"))
        },
        |_| Ok(()),
        PluginError::ExecutionError,
    )
    .unwrap_err();
    let second = call_with_quotas(
        &mut store,
        &mut availability,
        |_| {
            call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        },
        |_| Ok(()),
        PluginError::ExecutionError,
    );

    assert!(
        matches!(first, PluginError::ExecutionError(message) if message == "guest-owned secret")
    );
    assert!(second.is_ok());
    assert_eq!(call_entries.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(availability.reason(), None);
}

#[test]
fn oversized_result_is_terminal_redacted_and_second_call_skips_guest_boundary() {
    let runtime = WasmRuntime::new().unwrap();
    let host = StubHost::new();
    let mut store = new_plugin_store(&runtime.engine, host).unwrap();
    let mut availability = InstanceAvailability::default();
    let call_entries = std::sync::atomic::AtomicUsize::new(0);
    let secret = "guest-secret";

    let first = call_with_quotas(
        &mut store,
        &mut availability,
        |_| {
            call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(format!(
                "{secret}{}",
                "x".repeat(MAX_SERIALIZED_RESULT_BYTES)
            ))
        },
        validate_serialized_result,
        PluginError::ExecutionError,
    )
    .unwrap_err();
    let second = call_with_quotas(
        &mut store,
        &mut availability,
        |_| {
            call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(String::new())
        },
        validate_serialized_result,
        PluginError::ExecutionError,
    )
    .unwrap_err();

    assert!(
        matches!(first, PluginError::Unavailable(ref reason) if reason == "plugin result quota exceeded" && !reason.contains(secret))
    );
    assert!(matches!(second, PluginError::Unavailable(_)));
    assert_eq!(call_entries.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(availability.reason(), Some("plugin result quota exceeded"));
}
