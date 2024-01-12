use std::fs::File;

use anyhow::{bail, Context, Result};
use containerd_shim_wasm::container::{
    Engine, Entrypoint, Instance, RuntimeContext, Stdio, WasmBinaryType,
};
use wasi_common::I32Exit;
use wasmtime::component::{self as wasmtime_component, Component};
use wasmtime::{Module, Store};
use wasmtime_wasi::preview2::{self as wasi_preview2, Table};
use wasmtime_wasi::{self as wasi_preview1, Dir};

pub type WasmtimeInstance = Instance<WasmtimeEngine>;

#[derive(Clone)]
pub struct WasmtimeEngine {
    engine: wasmtime::Engine,
}

impl Default for WasmtimeEngine {
    fn default() -> Self {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true); // enable component linking
        Self {
            engine: wasmtime::Engine::new(&config)
                .context("failed to create wasmtime engine")
                .unwrap(),
        }
    }
}

/// Data that contains both wasi_preview1 and wasi_preview2 contexts.
pub struct WasiCtx {
    pub(crate) wasi_preview2: wasi_preview2::WasiCtx,
    pub(crate) wasi_preview1: wasi_preview1::WasiCtx,
    pub(crate) wasi_preview2_table: Table,
}

/// This impl is required to use wasmtime_wasi::preview2::WasiView trait.
impl wasmtime_wasi::preview2::WasiView for WasiCtx {
    fn table(&self) -> &Table {
        &self.wasi_preview2_table
    }

    fn table_mut(&mut self) -> &mut Table {
        &mut self.wasi_preview2_table
    }

    fn ctx(&self) -> &wasi_preview2::WasiCtx {
        &self.wasi_preview2
    }

    fn ctx_mut(&mut self) -> &mut wasi_preview2::WasiCtx {
        &mut self.wasi_preview2
    }
}

impl Engine for WasmtimeEngine {
    fn name() -> &'static str {
        "wasmtime"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext, wasm_bytes: &[u8], stdio: Stdio) -> Result<i32> {
        log::info!("setting up wasi");
        let envs: Vec<_> = std::env::vars().collect();
        let Entrypoint {
            source: _,
            func,
            arg0: _,
            name: _,
        } = ctx.entrypoint();

        stdio.redirect()?;

        log::info!("building wasi context");
        let wasi_ctx = prepare_wasi_ctx(ctx, envs)?;
        let store = Store::new(&self.engine, wasi_ctx);

        log::info!("wasi context ready");
        let status = match WasmBinaryType::from_bytes(wasm_bytes) {
            Some(WasmBinaryType::Module) => self.execute_module(wasm_bytes, store, &func)?,
            Some(WasmBinaryType::Component) => self.execute_component(wasm_bytes, store, func)?,
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

        Ok(status)
    }
}

impl WasmtimeEngine {
    /// Execute a wasm module.
    ///
    /// This function adds wasi_preview1 to the linker and can be utilized
    /// to execute a wasm module that uses wasi_preview1.
    fn execute_module(
        &self,
        wasm_binary: &[u8],
        mut store: Store<WasiCtx>,
        func: &String,
    ) -> Result<std::prelude::v1::Result<(), anyhow::Error>, anyhow::Error> {
        log::debug!("loading wasm module");
        let module = Module::from_binary(&self.engine, wasm_binary)?;
        let mut module_linker = wasmtime::Linker::new(&self.engine);

        wasi_preview1::add_to_linker(&mut module_linker, |s: &mut WasiCtx| &mut s.wasi_preview1)?;

        log::info!("instantiating instance");
        let instance: wasmtime::Instance = module_linker.instantiate(&mut store, &module)?;

        log::info!("getting start function");
        let start_func = instance
            .get_func(&mut store, func)
            .context("module does not have a WASI start function")?;

        log::debug!("running start function {func:?}");
        let status = start_func.call(&mut store, &[], &mut []);
        Ok(status)
    }

    /// Execute a wasm component.
    ///
    /// This function adds wasi_preview2 to the linker and can be utilized
    /// to execute a wasm component that uses wasi_preview2.
    fn execute_component(
        &self,
        wasm_binary: &[u8],
        mut store: Store<WasiCtx>,
        func: String,
    ) -> Result<std::prelude::v1::Result<(), anyhow::Error>, anyhow::Error> {
        log::debug!("loading wasm component");
        let component = Component::from_binary(&self.engine, wasm_binary)?;
        let mut linker = wasmtime_component::Linker::new(&self.engine);

        wasi_preview2::command::sync::add_to_linker(&mut linker)?;

        log::info!("instantiating component");

        // This is a adapter logic that converts wasip1 `_start` function to wasip2 `run` function.
        //
        // TODO: think about a better way to do this.
        if func == "_start" {
            let (command, _instance) = wasi_preview2::command::sync::Command::instantiate(
                &mut store, &component, &linker,
            )?;

            let status = command.wasi_cli_run().call_run(&mut store)?.map_err(|_| {
                anyhow::anyhow!("failed to run component targeting `wasi:cli/command` world")
            });
            Ok(status)
        } else {
            let instance = linker.instantiate(&mut store, &component)?;

            log::info!("getting component exported function {func:?}");
            let start_func = instance.get_func(&mut store, &func).context(format!(
                "component does not have exported function {func:?}"
            ))?;

            log::debug!("running exported function {func:?} {start_func:?}");
            let status = start_func.call(&mut store, &[], &mut []);
            Ok(status)
        }
    }
}

/// Prepare both wasi_preview1 and wasi_preview2 contexts.
fn prepare_wasi_ctx(
    ctx: &impl RuntimeContext,
    envs: Vec<(String, String)>,
) -> Result<WasiCtx, anyhow::Error> {
    let mut wasi_preview1_builder = wasi_preview1::WasiCtxBuilder::new();
    wasi_preview1_builder
        .args(ctx.args())?
        .envs(envs.as_slice())?
        .inherit_stdio()
        .preopened_dir(Dir::from_std_file(File::open("/")?), "/")?;
    let wasi_preview1_ctx = wasi_preview1_builder.build();

    // TODO: make this more configurable (e.g. allow the user to specify the
    // preopened directories and their permissions)
    // https://github.com/containerd/runwasi/issues/413
    let file_perms = wasi_preview2::FilePerms::all();
    let dir_perms = wasi_preview2::DirPerms::all();

    let mut wasi_preview2_builder = wasi_preview2::WasiCtxBuilder::new();
    wasi_preview2_builder
        .args(ctx.args())
        .envs(envs.as_slice())
        .inherit_stdio()
        .preopened_dir(
            Dir::from_std_file(File::open("/")?),
            dir_perms,
            file_perms,
            "/",
        );
    let wasi_preview2_ctx = wasi_preview2_builder.build();
    let wasi_data = WasiCtx {
        wasi_preview1: wasi_preview1_ctx,
        wasi_preview2: wasi_preview2_ctx,
        wasi_preview2_table: wasi_preview2::Table::new(),
    };
    Ok(wasi_data)
}
