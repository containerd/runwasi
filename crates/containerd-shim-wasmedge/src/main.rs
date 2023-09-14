use std::sync::OnceLock;

use anyhow::{Context, Result};
use containerd_shim_wasm::container::{RuntimeContext, Stdio};
use wasmedge_sdk::config::{ConfigBuilder, HostRegistrationConfigOptions};
use wasmedge_sdk::plugin::PluginManager;
use wasmedge_sdk::{Vm, VmBuilder};

static VM: OnceLock<Vm> = OnceLock::new();

#[containerd_shim_wasm::main("WasmEdge")]
fn main(ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
    let mut vm = VM
        .get_or_init(|| {
            PluginManager::load(None).unwrap();

            let host_options = HostRegistrationConfigOptions::default();
            let host_options = host_options.wasi(true);
            #[cfg(all(target_os = "linux", feature = "wasi_nn", target_arch = "x86_64"))]
            let host_options = host_options.wasi_nn(true);

            let config = ConfigBuilder::default()
                .with_host_registration_config(host_options)
                .build()
                .unwrap();
            VmBuilder::new().with_config(config).build().unwrap()
        })
        .clone();

    let args = ctx.args();
    let envs: Vec<_> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    let (path, func) = ctx
        .resolved_wasi_entrypoint()
        .context("module not found")?
        .into();

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

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmedge_tests;
