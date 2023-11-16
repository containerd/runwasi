use anyhow::{bail, Context, Result};
use containerd_shim_wasm::container::{
    Engine, Entrypoint, Instance, PathResolve, RuntimeContext, Source, Stdio,
};
use wasi_common::I32Exit;
use wasmtime::component::Component;
use wasmtime::{Module, Store};
use wasmtime_wasi::preview2::{self as wasi_preview2, Table};
use wasmtime_wasi::{Dir, WasiCtxBuilder};

pub type WasmtimeInstance = Instance<WasmtimeEngine>;

#[derive(Clone)]
pub struct WasmtimeEngine {
    engine: wasmtime::Engine,
}

impl Default for WasmtimeEngine {
    fn default() -> Self {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        Self {
            engine: wasmtime::Engine::new(&config)
                .context("failed to create wasmtime engine")
                .unwrap(),
        }
    }
}

pub struct Data {
    wasi: wasi_preview2::WasiCtx,
    table: Table,
}

impl wasmtime_wasi::preview2::WasiView for Data {
    fn table(&self) -> &Table {
        &self.table
    }

    fn table_mut(&mut self) -> &mut Table {
        &mut self.table
    }

    fn ctx(&self) -> &wasi_preview2::WasiCtx {
        &self.wasi
    }

    fn ctx_mut(&mut self) -> &mut wasi_preview2::WasiCtx {
        &mut self.wasi
    }
}

pub enum WasmBinaryType {
    Module,
    Component,
}

fn wasm_binary_type(bytes: &[u8]) -> Option<WasmBinaryType> {
    if bytes.starts_with(b"\0asm\x01\0\0\0") {
        return Some(WasmBinaryType::Module);
    }
    if bytes.starts_with(b"\0asm\x0d\0\x01\0") {
        return Some(WasmBinaryType::Component);
    }
    None
}

impl Engine for WasmtimeEngine {
    fn name() -> &'static str {
        "wasmtime"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
        log::info!("setting up wasi");
        let root_path = Dir::from_std_file(std::fs::File::open("/")?);
        let envs: Vec<_> = std::env::vars().collect();
        let Entrypoint {
            source,
            func,
            arg0: _,
            name: _,
        } = ctx.entrypoint();

        let data = match source {
            Source::File(path) => {
                log::info!("loading module from file");
                let path = path
                    .resolve_in_path_or_cwd()
                    .next()
                    .context("module not found")?;
                std::fs::read(path)?
            }
            Source::Oci([module]) => {
                log::info!("loading module wasm OCI layers");
                module.layer.clone()
            }
            Source::Oci(_modules) => {
                bail!("only a single module is supported when using images with OCI layers")
            }
        };

        let status = match wasm_binary_type(&data) {
            Some(WasmBinaryType::Module) => {
                let module = Module::from_binary(&self.engine, &data)?;
                let mut wasi_builder = WasiCtxBuilder::new();
                wasi_builder
                    .args(ctx.args())?
                    .envs(envs.as_slice())?
                    .inherit_stdio()
                    .preopened_dir(root_path, "/")?;

                stdio.redirect()?;

                log::info!("building wasi context");
                let wctx = wasi_builder.build();

                let mut module_linker = wasmtime::Linker::new(&self.engine);

                wasmtime_wasi::add_to_linker(&mut module_linker, |s| s)?;
                let mut store = Store::new(&self.engine, wctx);

                log::info!("instantiating instance");
                let instance: wasmtime::Instance =
                    module_linker.instantiate(&mut store, &module)?;

                log::info!("getting start function");
                let start_func = instance
                    .get_func(&mut store, &func)
                    .context("module does not have a WASI start function")?;

                log::debug!("running start function {func:?}");

                let status = start_func.call(&mut store, &[], &mut []);
                status
            }
            Some(WasmBinaryType::Component) => {
                let component = Component::from_binary(&self.engine, &data)?;
                let file_perms = wasi_preview2::FilePerms::all();
                let dir_perms = wasi_preview2::DirPerms::all();
                let mut wasi_builder = wasi_preview2::WasiCtxBuilder::new();
                wasi_builder
                    .args(ctx.args())
                    .envs(envs.as_slice())
                    .inherit_stdio()
                    .preopened_dir(root_path, dir_perms, file_perms, "/");

                stdio.redirect()?;

                log::info!("building wasi context");
                let wctx = wasi_builder.build();

                let data = Data {
                    wasi: wctx,
                    table: wasmtime_wasi::preview2::Table::new(),
                };

                let mut linker = wasmtime::component::Linker::new(&self.engine);

                wasmtime_wasi::preview2::command::add_to_linker(&mut linker)?;
                let mut store = Store::new(&self.engine, data);

                log::info!("instantiating instance");
                let instance: wasmtime::component::Instance =
                    linker.instantiate(&mut store, &component)?;

                log::info!("getting start function");
                let start_func = instance
                    .get_func(&mut store, &func)
                    .context("module does not have a WASI start function")?;

                log::debug!("running start function {func:?}");

                let status = start_func.call(&mut store, &[], &mut []);
                status
            }
            None => bail!("not a valid wasm binary format"),
        };

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
        log::info!("wasi context ready");

        Ok(status)
    }
}
