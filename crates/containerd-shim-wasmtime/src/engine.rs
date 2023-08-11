use containerd_shim_wasm::sandbox::instance::Engine as InstanceEngine;
use wasmtime::Engine;

#[derive(Default, Clone)]
pub struct WasmtimeEngine {
    engine: Engine,
}

impl From<Engine> for WasmtimeEngine {
    fn from(engine: Engine) -> Self {
        Self { engine }
    }
}

impl From<WasmtimeEngine> for Engine {
    fn from(val: WasmtimeEngine) -> Self {
        val.engine
    }
}

impl InstanceEngine for WasmtimeEngine {
    fn new() -> Self {
        Self {
            engine: Engine::default(),
        }
    }
}
