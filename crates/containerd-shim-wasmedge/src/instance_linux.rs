use anyhow::{Context, Result};
use containerd_shim_wasm::container::{Engine, Instance, RuntimeContext};
use containerd_shim_wasm::sandbox::Stdio;
use wasmedge_sdk::config::{ConfigBuilder, HostRegistrationConfigOptions};
use wasmedge_sdk::plugin::PluginManager;
use wasmedge_sdk::VmBuilder;

const ENGINE_NAME: &str = "wasmedge";

pub type WasmEdgeInstance = Instance<WasmEdgeEngine>;

#[derive(Clone)]
pub struct WasmEdgeEngine {
    vm: wasmedge_sdk::Vm,
}

impl Default for WasmEdgeEngine {
    fn default() -> Self {
        PluginManager::load(None).unwrap();

        let host_options = HostRegistrationConfigOptions::default();
        let host_options = host_options.wasi(true);
        #[cfg(all(target_os = "linux", feature = "wasi_nn", target_arch = "x86_64"))]
        let host_options = host_options.wasi_nn(true);

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
        ENGINE_NAME
    }

    fn run(&self, ctx: impl RuntimeContext, stdio: Stdio) -> Result<i32> {
        let args = ctx.args();
        let (module, method) = ctx.module();
        let envs: Vec<_> = ctx.envs().map(|(k, v)| format!("{k}={v}")).collect();

        let mut vm = self.vm.clone();
        vm.wasi_module_mut()
            .context("Not found wasi module")?
            .initialize(
                Some(args.iter().map(String::as_str).collect()),
                Some(envs.iter().map(String::as_str).collect()),
                None,
            );

        let vm = vm.register_module_from_file("main", module)?;

        stdio.redirect()?;

        // TODO: How to get exit code?
        // This was relatively straight forward in go, but wasi and wasmtime are totally separate things in rust
        log::debug!("running {:?} with method {}", module, method);
        vm.run_func(Some("main"), method, vec![])?;

        let status = vm
            .wasi_module()
            .context("Not found wasi module")?
            .exit_code();

        Ok(status as i32)
    }
}
