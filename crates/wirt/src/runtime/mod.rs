//! Generic Wasmtime runtime for product-neutral Wirt plugin components.

mod epoch;
mod instance;
pub(crate) mod quota;
#[cfg(test)]
mod tests;

use crate::limits::PluginStoreLimiter;
use crate::{PluginError, Result};
use epoch::EpochTicker;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info};
use wasmtime::component::Component;
use wasmtime::{Config, Engine};
use wasmtime_wasi::clocks::HostWallClock;
use wasmtime_wasi::random::Deterministic;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder};

pub use instance::PluginInstance;

struct FixedWallClock;

impl HostWallClock for FixedWallClock {
    fn resolution(&self) -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }

    fn now(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
}

/// Construct Wirt's fixed-authority WASI context.
///
/// Standard I/O is closed or discarded, environment/arguments/preopens are
/// empty, the wall clock is fixed at the Unix epoch, and both random streams
/// are deterministic. The real monotonic clock remains available for bounded
/// guest scheduling and deadline support.
pub fn sandboxed_wasi_ctx() -> WasiCtx {
    let mut builder = WasiCtxBuilder::new();
    builder
        .wall_clock(FixedWallClock)
        .secure_random(Deterministic::new(vec![0; 32]))
        .insecure_random(Deterministic::new(vec![0; 32]))
        .insecure_random_seed(0);
    builder.build()
}

/// Store state required by the Wirt runtime boundary.
pub trait WirtStoreState:
    wasmtime_wasi::WasiView
    + crate::bindings::wirt::plugin::host::Host
    + crate::bindings::wirt::plugin::ui::Host
    + crate::bindings::wirt::plugin::rules::Host
    + crate::bindings::wirt::plugin::meta::Host
    + Send
    + 'static
{
    fn store_limiter(&mut self) -> &mut PluginStoreLimiter;
}

/// Wasmtime engine and epoch ticker shared by loaded plugin components.
pub struct WasmRuntime {
    engine: Engine,
    epoch_ticker: Arc<EpochTicker>,
}

impl WasmRuntime {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);

        let engine =
            Engine::new(&config).map_err(|error| PluginError::WasmError(error.to_string()))?;
        let epoch_ticker = Arc::new(EpochTicker::spawn(engine.clone()).map_err(|_| {
            PluginError::WasmError("failed to start WASM epoch ticker".to_string())
        })?);

        info!("WASM runtime initialized (Component Model enabled)");
        Ok(Self {
            engine,
            epoch_ticker,
        })
    }

    #[cfg(test)]
    pub(super) fn epoch_ticker_exit_probe(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.epoch_ticker.exited.clone()
    }

    pub fn load_component(&self, id: String, path: &Path) -> Result<LoadedComponent> {
        debug!("Loading WASM component from: {}", path.display());
        let component = Component::from_file(&self.engine, path).map_err(|error| {
            PluginError::LoadError(format!("Failed to load component: {error}"))
        })?;
        info!("WASM component loaded successfully: {}", path.display());

        Ok(LoadedComponent {
            id,
            component,
            engine: self.engine.clone(),
            epoch_ticker: self.epoch_ticker.clone(),
            _path: path.to_path_buf(),
        })
    }

    pub fn load_component_from_bytes(&self, id: String, bytes: &[u8]) -> Result<LoadedComponent> {
        debug!("Loading WASM component from bytes ({} bytes)", bytes.len());
        let component = Component::from_binary(&self.engine, bytes)
            .map_err(|error| PluginError::LoadError(error.to_string()))?;
        info!("WASM component loaded successfully from bytes");

        Ok(LoadedComponent {
            id,
            component,
            engine: self.engine.clone(),
            epoch_ticker: self.epoch_ticker.clone(),
            _path: PathBuf::from("<bytes>"),
        })
    }
}

/// Compiled component ready to instantiate with any neutral Wirt host state.
pub struct LoadedComponent {
    id: String,
    component: Component,
    engine: Engine,
    epoch_ticker: Arc<EpochTicker>,
    _path: PathBuf,
}

impl LoadedComponent {
    pub fn id(&self) -> &str {
        &self.id
    }
}
