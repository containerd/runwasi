use anyhow::{Context, Result};
use containerd_shim_wasm::container::{
    Engine, Instance, PathResolve, RuntimeContext, Stdio, WasiEntrypoint,
};
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
        "wasmedge"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
        let args = ctx.args();
        let envs: Vec<_> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
        let WasiEntrypoint { path, func } = ctx.wasi_entrypoint();
        let path = path
            .resolve_in_path_or_cwd()
            .next()
            .context("module not found")?;

        let mut vm = self.vm.clone();
        vm.wasi_module_mut()
            .context("Not found wasi module")?
            .initialize(
                Some(args.iter().map(String::as_str).collect()),
                Some(envs.iter().map(String::as_str).collect()),
                Some(vec!["/:/"]),
            );

        let mod_name = match path.file_stem() {
            Some(name) => name.to_string_lossy().to_string(),
            None => "main".to_string(),
        };

        let vm = vm
            .register_module_from_file(&mod_name, &path)
            .context("registering module")?;

        stdio.redirect()?;

        log::debug!("running {path:?} with method {func:?}");
        vm.run_func(Some(&mod_name), func, vec![])?;

        let status = vm
            .wasi_module()
            .context("Not found wasi module")?
            .exit_code();

        Ok(status as i32)
    }
}
