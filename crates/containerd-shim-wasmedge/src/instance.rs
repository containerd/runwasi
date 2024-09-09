use anyhow::{Context, Result};
use containerd_shim_wasm::container::{Engine, Entrypoint, Instance, RuntimeContext, Stdio};
use wasmedge_sdk::config::{ConfigBuilder, HostRegistrationConfigOptions};
use wasmedge_sdk::plugin::PluginManager;
use wasmedge_sdk::VmBuilder;

pub type WasmEdgeInstance = Instance<WasmEdgeEngine>;

#[derive(Clone)]
pub struct WasmEdgeEngine {
    vm: wasmedge_sdk::Vm,
}

impl Default for WasmEdgeEngine {
    fn default() -> Self {
        let host_options = HostRegistrationConfigOptions::default();
        let host_options = host_options.wasi(true);
        let config = ConfigBuilder::default()
            .with_host_registration_config(host_options)
            .build()
            .unwrap();
        let vm = VmBuilder::new().with_config(config).build().unwrap();
        Self { vm }
    }
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

        let mut vm = self.vm.clone();
        vm.wasi_module_mut()
            .context("Not found wasi module")?
            .initialize(
                Some(args.iter().map(String::as_str).collect()),
                Some(envs.iter().map(String::as_str).collect()),
                Some(vec!["/:/"]),
            );

        let mod_name = name.unwrap_or_else(|| "main".to_string());

        PluginManager::load(None)?;
        let vm = vm.auto_detect_plugins()?;

        let wasm_bytes = source.as_bytes()?;
        let vm = vm
            .register_module_from_bytes(&mod_name, wasm_bytes)
            .context("registering module")?;

        stdio.redirect()?;

        log::debug!("running with method {func:?}");
        vm.run_func(Some(&mod_name), func, vec![])?;

        let status = vm
            .wasi_module()
            .context("Not found wasi module")?
            .exit_code();

        Ok(status as i32)
    }
}
