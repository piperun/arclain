use std::sync::Arc;
use std::time::Duration;
use wasmtime::Engine;

// Fuel is the deterministic export work budget. The epoch deadline is only a
// liveness dead-man switch for guests that repeatedly cross cheap hostcalls:
// fuel cannot see time spent on the host side, while epoch checks run at guest
// function entries and loop backedges. Individual hostcalls must still enforce
// their own timeouts because an epoch trap cannot interrupt host code that does
// not return.
//
// An epoch trap permanently poisons the component instance, so this ceiling is
// deliberately measured in minutes and must dwarf legitimate exports that make
// several sequential bounded hostcalls. Do not tune it toward the fuel budget.
pub(super) const EPOCH_TICKS_PER_EXPORT: u64 = 30_000;
pub(super) const EPOCH_TICK_INTERVAL: Duration = Duration::from_millis(10);

pub(super) struct EpochTicker {
    control: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
    #[cfg(test)]
    pub(super) exited: Arc<std::sync::atomic::AtomicBool>,
}

impl EpochTicker {
    pub(super) fn spawn(engine: Engine) -> std::io::Result<Self> {
        let control = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let thread_control = control.clone();
        #[cfg(test)]
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        #[cfg(test)]
        let thread_exited = exited.clone();
        let worker = std::thread::Builder::new()
            .name("wirt-wasm-epoch".to_string())
            .spawn(move || {
                let (stop, wake) = &*thread_control;
                let mut stopped = stop.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                loop {
                    let (next_stop, wait) = wake
                        .wait_timeout(stopped, EPOCH_TICK_INTERVAL)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    stopped = next_stop;
                    if *stopped {
                        break;
                    }
                    if wait.timed_out() {
                        engine.increment_epoch();
                    }
                }
                #[cfg(test)]
                thread_exited.store(true, std::sync::atomic::Ordering::Release);
            })?;

        Ok(Self {
            control,
            worker: Some(worker),
            #[cfg(test)]
            exited,
        })
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        let (stop, wake) = &*self.control;
        *stop.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        wake.notify_one();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                tracing::warn!("WASM epoch ticker terminated unexpectedly");
            }
        }
    }
}
