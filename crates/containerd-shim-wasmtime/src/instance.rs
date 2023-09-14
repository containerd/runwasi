use anyhow::{Context, Result};
use containerd_shim_wasm::container::{
    Engine, Instance, PathResolve, RuntimeContext, Stdio, WasiEntrypoint,
};
use wasi_common::I32Exit;
use wasmtime::{Linker, Module, Store};
use wasmtime_wasi::{Dir, WasiCtxBuilder};

pub type WasmtimeInstance = Instance<WasmtimeEngine>;

#[derive(Clone, Default)]
pub struct WasmtimeEngine {
    engine: wasmtime::Engine,
}

impl Engine for WasmtimeEngine {
    fn name() -> &'static str {
        "wasmtime"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
        log::info!("setting up wasi");
        let path = Dir::from_std_file(std::fs::File::open("/")?);
        let envs: Vec<_> = std::env::vars().collect();

        let wasi_builder = WasiCtxBuilder::new()
            .args(ctx.args())?
            .envs(envs.as_slice())?
            .inherit_stdio()
            .preopened_dir(path, "/")?;

        stdio.redirect()?;

        log::info!("building wasi context");
        let wctx = wasi_builder.build();

        log::info!("wasi context ready");
        let WasiEntrypoint { path, func } = ctx.wasi_entrypoint();
        let path = path
            .resolve_in_path_or_cwd()
            .next()
            .context("module not found")?;

        log::info!("loading module from file {path:?}");
        let module = Module::from_file(&self.engine, &path)?;
        let mut linker = Linker::new(&self.engine);

        wasmtime_wasi::add_to_linker(&mut linker, |s| s)?;
        let mut store = Store::new(&self.engine, wctx);

        log::info!("instantiating instance");
        let instance: wasmtime::Instance = linker.instantiate(&mut store, &module)?;

        log::info!("getting start function");
        let start_func = instance
            .get_func(&mut store, &func)
            .context("module does not have a WASI start function")?;

        log::debug!("running {path:?} with start function {func:?}");

        let status = start_func.call(&mut store, &[], &mut []);
        let status = status.map(|_| 0).or_else(|err| {
            match err.downcast_ref::<I32Exit>() {
                // On Windows, exit status 3 indicates an abort (see below),
                // so return 1 indicating a non-zero status to avoid ambiguity.
                #[cfg(windows)]
                Some(I32Exit(3..)) => Ok(1),
                Some(I32Exit(status)) => Ok(*status),
                _ => Err(err),
            }
        })?;

        Ok(status)
    }
}
