use std::collections::HashMap;

use anyhow::{Context, Result};
use containerd_shim_wasm::container::{Engine, Entrypoint, Instance, RuntimeContext, Stdio};
use wasmedge_sdk::config::Config;
use wasmedge_sdk::plugin::PluginManager;
use wasmedge_sdk::wasi::WasiModule;
use wasmedge_sdk::{Module, Store, Vm};

pub type WasmEdgeInstance = Instance<WasmEdgeEngine>;

#[derive(Clone, Default)]
pub struct WasmEdgeEngine {
    config: Option<Config>,
}

impl Engine for WasmEdgeEngine {
    fn name() -> &'static str {
        "wasmedge"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
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

        let mut instances = HashMap::new();
        instances.insert(wasi_module.name().to_string(), wasi_module.as_mut());
        let mut vm = Vm::new(Store::new(self.config.as_ref(), instances).unwrap());
        let mod_name = name.unwrap_or_else(|| "main".to_string());

        PluginManager::load(None)?;
        PluginManager::auto_detect_plugins()?;

        let wasm_bytes = source.as_bytes()?;
        let module = Module::from_bytes(None, &wasm_bytes)?;
        let vm = vm
            .register_module(Some(&mod_name), module)
            .context("registering module")?;

        stdio.redirect()?;

        log::debug!("running with method {func:?}");
        vm.run_func(Some(&mod_name), func, vec![])?;

        Ok(wasi_module.exit_code() as i32)
    }
}
