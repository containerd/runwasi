use anyhow::{Context, Result};
use containerd_shim_wasm::container::{Engine, Instance, RuntimeContext};
use containerd_shim_wasm::sandbox::Stdio;
use wasmtime::{Linker, Module, Store};
use wasmtime_wasi::{maybe_exit_on_error, Dir, WasiCtxBuilder};

const ENGINE_NAME: &str = "wasmtime";

pub type WasmtimeInstance = Instance<WasmtimeEngine>;

#[derive(Clone, Default)]
pub struct WasmtimeEngine {
    engine: wasmtime::Engine,
}

impl Engine for WasmtimeEngine {
    fn name() -> &'static str {
        ENGINE_NAME
    }

    fn run(&self, ctx: impl RuntimeContext, stdio: Stdio) -> Result<i32> {
        log::info!("setting up wasi");
        let path = Dir::from_std_file(std::fs::File::open(".")?);
        let envs = ctx
            .envs()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect::<Vec<_>>();

        let wasi_builder = WasiCtxBuilder::new()
            .args(ctx.args())?
            .envs(envs.as_slice())?
            .inherit_stdio()
            .preopened_dir(path, "/")?;

        stdio.redirect()?;

        log::info!("building wasi context");
        let wctx = wasi_builder.build();

        log::info!("wasi context ready");
        let (module, method) = ctx.module();

        log::info!("loading module from file {module:?}");
        let module = Module::from_file(&self.engine, module)?;
        let mut linker = Linker::new(&self.engine);

        wasmtime_wasi::add_to_linker(&mut linker, |s| s)?;
        let mut store = Store::new(&self.engine, wctx);

        log::info!("instantiating instance");
        let instance: wasmtime::Instance = linker.instantiate(&mut store, &module)?;

        log::info!("getting start function");
        let start_func = instance
            .get_func(&mut store, method)
            .context("module does not have a WASI start function")?;

        log::info!("calling start function");

        let status = start_func.call(&mut store, &[], &mut []);
        let status = status.map(|_| 0).map_err(maybe_exit_on_error)?;

        Ok(status)
    }
}
