use anyhow::{bail, Context, Result};
use containerd_shim_wasm::container::{
    Engine, Instance, PathResolve, RuntimeContext, Stdio, WasiEntrypoint, WasiLoadingStrategy,
};
use log::debug;
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
        let envs: Vec<_> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
        let WasiEntrypoint {
            path: entrypoint_path,
            func,
            arg0: _,
        } = ctx.entrypoint();

        let mut vm = self.vm.clone();
        vm.wasi_module_mut()
            .context("Not found wasi module")?
            .initialize(
                Some(args.iter().map(String::as_str).collect()),
                Some(envs.iter().map(String::as_str).collect()),
                Some(vec!["/:/"]),
            );

        let mod_name = match entrypoint_path.file_stem() {
            Some(name) => name.to_string_lossy().to_string(),
            None => "main".to_string(),
        };

        PluginManager::load(None)?;
        let vm = vm.auto_detect_plugins()?;

        let vm = match ctx.wasi_loading_strategy() {
            WasiLoadingStrategy::File(path) => {
                debug!("loading module from file {path:?}");
                let path = path
                    .resolve_in_path_or_cwd()
                    .next()
                    .context("module not found")?;

                vm.register_module_from_file(&mod_name, path)
                    .context("registering module")?
            }
            WasiLoadingStrategy::Oci([module]) => {
                log::info!("loading module from wasm OCI layers");
                vm.register_module_from_bytes(&mod_name, &module.layer)
                    .context("registering module")?
            }
            WasiLoadingStrategy::Oci(_modules) => {
                bail!("only a single module is supported when using images with OCI layers")
            }
        };

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
