use std::collections::HashMap;
use std::env;
#[cfg(all(feature = "plugin", not(target_env = "musl")))]
use std::str::FromStr;

use anyhow::{Context, Result};
use cfg_if::cfg_if;
use containerd_shim_wasm::container::{Entrypoint, RuntimeContext, Sandbox, Shim};
#[cfg(all(feature = "plugin", not(target_env = "musl")))]
use wasmedge_sdk::AsInstance;
use wasmedge_sdk::config::{CommonConfigOptions, Config, ConfigBuilder};
#[cfg(all(feature = "plugin", not(target_env = "musl")))]
use wasmedge_sdk::plugin::NNPreload;
#[cfg(all(feature = "plugin", not(target_env = "musl")))]
use wasmedge_sdk::plugin::PluginManager;
use wasmedge_sdk::wasi::WasiModule;
use wasmedge_sdk::{Module, Store, Vm};

pub struct WasmEdgeShim;

pub struct WasmEdgeSandbox {
    config: Config,
}

impl Default for WasmEdgeSandbox {
    fn default() -> Self {
        let config = ConfigBuilder::new(CommonConfigOptions::default())
            .build()
            .expect("failed to create config");
        Self { config }
    }
}

impl Shim for WasmEdgeShim {
    fn name() -> &'static str {
        "wasmedge"
    }

    type Sandbox = WasmEdgeSandbox;
}

impl Sandbox for WasmEdgeSandbox {
    async fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32> {
        let args = ctx.args();
        let envs = ctx.envs();
        let Entrypoint {
            source,
            func,
            arg0: _,
            name,
        } = ctx.entrypoint();

        containerd_shim_wasm::debug!(ctx, "initializing WasmEdge runtime");

        let prefix = "WASMEDGE_";
        for env in envs.iter().filter(|env| env.starts_with(prefix)) {
            if let Some((key, value)) = env.split_once('=') {
                unsafe {
                    env::set_var(key, value);
                }
            }
        }

        let mut instances = HashMap::new();
        cfg_if! {
            if #[cfg(all(feature = "plugin", not(target_env = "musl")))] {
                PluginManager::load(None)?;
                match env::var("WASMEDGE_WASINN_PRELOAD") {
                    Ok(value) => PluginManager::nn_preload(vec![NNPreload::from_str(value.as_str())?]),
                    Err(_) => log::debug!("No specific nn_preload parameter for wasi_nn plugin"),
                }

                // Load the wasi_nn plugin manually as a workaround.
                // It should call auto_detect_plugins after the issue is fixed.
                let mut wasi_nn = PluginManager::names()
                    .contains(&"wasi_nn".to_string())
                    .then(PluginManager::load_plugin_wasi_nn)
                    .transpose()?;
                if let Some(ref mut nn) = wasi_nn {
                    instances.insert(nn.name().unwrap().to_string(), nn);
                }
            }
        }

        let mut wasi_module = WasiModule::create(
            Some(args.iter().map(String::as_str).collect()),
            Some(envs.iter().map(String::as_str).collect()),
            Some(vec!["/:/"]),
        )?;
        instances.insert(wasi_module.name().to_string(), wasi_module.as_mut());

        let wasm_bytes = source.as_bytes()?;
        let module = Module::from_bytes(Some(&self.config), &wasm_bytes)?;
        let mut vm = Vm::new(Store::new(Some(&self.config), instances).unwrap());
        let mod_name = name.unwrap_or_else(|| "main".to_string());

        let vm = vm
            .register_module(Some(&mod_name), module)
            .context("registering module")?;

        containerd_shim_wasm::debug!(ctx, "running with method {func:?}");
        vm.run_func(Some(&mod_name), func, vec![])?;

        Ok(wasi_module.exit_code() as i32)
    }
}
