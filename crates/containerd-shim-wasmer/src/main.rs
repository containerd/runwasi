use anyhow::{Context, Result};
use containerd_shim_wasm::container::{RuntimeContext, Stdio};
use wasmer::{Cranelift, Module, Store};
use wasmer_wasix::virtual_fs::host_fs::FileSystem;
use wasmer_wasix::{WasiEnv, WasiError};

#[containerd_shim_wasm::main("Wasmer")]
async fn main(ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
    let args = ctx.args();
    let envs = std::env::vars();
    let (path, func) = ctx
        .resolved_wasi_entrypoint()
        .context("module not found")?
        .into();

    let mod_name = match path.file_stem() {
        Some(name) => name.to_string_lossy().to_string(),
        None => "main".to_string(),
    };

    log::info!("redirect stdio");
    stdio.redirect()?;

    log::info!("Create a Store");
    let mut store = Store::new(Cranelift::new());

    log::info!("loading module from file {path:?}");
    let module = Module::from_file(&store, path)?;

    log::info!("Creating `WasiEnv`...: args {args:?}, envs: {envs:?}");
    let (instance, wasi_env) = WasiEnv::builder(mod_name)
        .args(&args[1..])
        .envs(envs)
        .fs(Box::<FileSystem>::default())
        .preopen_dir("/")?
        .instantiate(module, &mut store)?;

    log::info!("Running {func:?}");
    let start = instance.exports.get_function(&func)?;
    wasi_env.data(&store).thread.set_status_running();
    let status = start.call(&mut store, &[]).map(|_| 0).or_else(|err| {
        match err.downcast_ref::<WasiError>() {
            Some(WasiError::Exit(code)) => Ok(code.raw()),
            _ => Err(err),
        }
    })?;

    Ok(status)
}

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmer_tests;
