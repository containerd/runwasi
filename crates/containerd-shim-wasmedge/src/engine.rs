use containerd_shim_wasm::sandbox::instance::Engine;
use wasmedge_sdk::config::CommonConfigOptions;
use wasmedge_sdk::config::ConfigBuilder;
use wasmedge_sdk::config::HostRegistrationConfigOptions;
use wasmedge_sdk::Vm;
use wasmedge_sdk::VmBuilder;

#[derive(Debug, Clone)]
pub struct WasmEdgeEngine {
    vm: Vm,
}

impl From<WasmEdgeEngine> for Vm {
    fn from(val: WasmEdgeEngine) -> Self {
        val.vm
    }
}

impl From<Vm> for WasmEdgeEngine {
    fn from(vm: Vm) -> Self {
        Self { vm }
    }
}

impl Default for WasmEdgeEngine {
    fn default() -> Self {
        let config = ConfigBuilder::new(CommonConfigOptions::default())
            .with_host_registration_config(HostRegistrationConfigOptions::default().wasi(true))
            .build()
            .unwrap();
        let vm = VmBuilder::new().with_config(config).build().unwrap();
        Self { vm }
    }
}

impl Engine for WasmEdgeEngine {
    fn new() -> Self {
        WasmEdgeEngine::default()
    }
}
