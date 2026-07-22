use arclain_plugins::PluginManager;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tempfile::TempDir;

fn installed_ui_demo() -> (TempDir, PluginManager) {
    let temp_dir = TempDir::new().unwrap();
    let wasm_path = temp_dir.path().join("ui-demo.wasm");
    std::fs::write(
        &wasm_path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/ui-demo/ui-demo.wasm"
        )),
    )
    .unwrap();

    let mut manager = PluginManager::new(temp_dir.path().join("plugins"), HashMap::new()).unwrap();
    manager.install_plugin(&wasm_path).unwrap();
    (temp_dir, manager)
}

#[test]
fn enabled_snapshot_completes_reads_while_the_outer_manager_is_locked() {
    let (_temp_dir, manager) = installed_ui_demo();
    let manager = Arc::new(Mutex::new(manager));
    let snapshot = { manager.lock().enabled_plugin_snapshot() };
    assert_eq!(snapshot.plugin_ids().collect::<Vec<_>>(), ["ui-demo"]);

    let outer_guard = manager.lock();
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        sender
            .send((snapshot.get_all_top_tabs(), snapshot.get_network_log()))
            .expect("test receiver must remain connected");
    });

    let (_tabs, _network_log) = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("detached reads must not reacquire the outer manager mutex");
    drop(outer_guard);
    worker.join().unwrap();
}

#[test]
fn enabled_snapshot_network_log_waits_for_a_busy_plugin() {
    let (_temp_dir, manager) = installed_ui_demo();
    let snapshot = manager.enabled_plugin_snapshot();
    let instance = snapshot
        .instance("ui-demo")
        .expect("enabled snapshot must expose the plugin instance handle");
    let instance_guard = instance.lock();

    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        sender
            .send(snapshot.get_network_log())
            .expect("test receiver must remain connected");
    });

    assert!(
        matches!(
            receiver.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "a complete snapshot must wait for a busy enabled plugin",
    );

    drop(instance_guard);
    receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("network-log aggregation must finish after the instance unlocks");
    worker.join().unwrap();
}

#[test]
fn enabled_snapshot_excludes_disabled_plugins() {
    let (_temp_dir, manager) = installed_ui_demo();
    manager.disable_plugin("ui-demo").unwrap();

    let snapshot = manager.enabled_plugin_snapshot();

    assert!(snapshot.is_empty());
    assert!(snapshot.instance("ui-demo").is_none());
}
