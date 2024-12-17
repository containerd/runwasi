use std::collections::HashMap;

use anyhow::{Context, Result};
use containerd_shim_wasm::container::{Engine, Entrypoint, Instance, RuntimeContext};
use wasmedge_sdk::config::{CommonConfigOptions, Config, ConfigBuilder};
use wasmedge_sdk::wasi::WasiModule;
use wasmedge_sdk::{Module, Store, Vm};

pub type WasmEdgeInstance = Instance<WasmEdgeEngine>;

#[derive(Clone)]
pub struct WasmEdgeEngine {
    config: Config,
}

impl Default for WasmEdgeEngine {
    fn default() -> Self {
        let config = ConfigBuilder::new(CommonConfigOptions::default())
            .build()
            .expect("failed to create config");
        Self { config }
    }
}

impl Engine for WasmEdgeEngine {
    fn name() -> &'static str {
        "wasmedge"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32> {
        let args = ctx.args();
        let envs = ctx.envs();
        let Entrypoint {
            source,
            func,
            arg0: _,
            name,
        } = ctx.entrypoint();

        let mut wasi_module = WasiModule::create(
            Some(args.iter().map(String::as_str).collect()),
            Some(envs.iter().map(String::as_str).collect()),
            Some(vec!["/:/"]),
        )?;
        let wasm_bytes = source.as_bytes()?;
        let module = Module::from_bytes(Some(&self.config), &wasm_bytes)?;

        let mut instances = HashMap::new();
        instances.insert(wasi_module.name().to_string(), wasi_module.as_mut());
        let mut vm = Vm::new(Store::new(Some(&self.config), instances).unwrap());
        let mod_name = name.unwrap_or_else(|| "main".to_string());

        let vm = vm
            .register_module(Some(&mod_name), module)
            .context("registering module")?;

        log::debug!("running with method {func:?}");
        vm.run_func(Some(&mod_name), func, vec![])?;

        Ok(wasi_module.exit_code() as i32)
    }
}
