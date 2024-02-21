use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use anyhow::{bail, Context, Result};
use containerd_shim_wasm::container::{
    Engine, Entrypoint, Instance, RuntimeContext, Stdio, WasmBinaryType,
};
use containerd_shim_wasm::sandbox::WasmLayer;
use wasi_common::I32Exit;
use wasmtime::component::{self as wasmtime_component, Component, ResourceTable};
use wasmtime::{Config, Module, Precompiled, Store};
use wasmtime_wasi::preview2::{self as wasi_preview2};
use wasmtime_wasi::{self as wasi_preview1, Dir};

pub type WasmtimeInstance = Instance<WasmtimeEngine<DefaultConfig>>;

#[derive(Clone)]
pub struct WasmtimeEngine<T: WasiConfig> {
    engine: wasmtime::Engine,
    config_type: PhantomData<T>,
}

#[derive(Clone)]
pub struct DefaultConfig {}

impl WasiConfig for DefaultConfig {
    fn new_config() -> Config {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true); // enable component linking
        config
    }
}

pub trait WasiConfig: Clone + Sync + Send + 'static {
    fn new_config() -> Config;
}

impl<T: WasiConfig> Default for WasmtimeEngine<T> {
    fn default() -> Self {
        let config = T::new_config();
        Self {
            engine: wasmtime::Engine::new(&config)
                .context("failed to create wasmtime engine")
                .unwrap(),
            config_type: PhantomData,
        }
    }
}

/// Data that contains both wasi_preview1 and wasi_preview2 contexts.
pub struct WasiCtx {
    pub(crate) wasi_preview2: wasi_preview2::WasiCtx,
    pub(crate) wasi_preview1: wasi_preview1::WasiCtx,
    pub(crate) resource_table: ResourceTable,
}

/// This impl is required to use wasmtime_wasi::preview2::WasiView trait.
impl wasmtime_wasi::preview2::WasiView for WasiCtx {
    fn table(&self) -> &ResourceTable {
        &self.resource_table
    }

    fn table_mut(&mut self) -> &mut ResourceTable {
        &mut self.resource_table
    }

    fn ctx(&self) -> &wasi_preview2::WasiCtx {
        &self.wasi_preview2
    }

    fn ctx_mut(&mut self) -> &mut wasi_preview2::WasiCtx {
        &mut self.wasi_preview2
    }
}

impl<T: WasiConfig> Engine for WasmtimeEngine<T> {
    fn name() -> &'static str {
        "wasmtime"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
        log::info!("setting up wasi");
        let envs: Vec<_> = std::env::vars().collect();
        let Entrypoint {
            source,
            func,
            arg0: _,
            name: _,
        } = ctx.entrypoint();

        stdio.redirect()?;

        log::info!("building wasi context");
        let wasi_ctx = prepare_wasi_ctx(ctx, envs)?;
        let store = Store::new(&self.engine, wasi_ctx);

        let wasm_bytes = &source.as_bytes()?;
        let status = self.execute(wasm_bytes, store, func)?;

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

    fn precompile(&self, layer: &WasmLayer) -> Option<Result<Vec<u8>>> {
         Some(self.engine.precompile_module(&layer.layer))
    }

    fn can_precompile(&self) -> Option<String> {
        let mut hasher = DefaultHasher::new();
        self.engine
            .precompile_compatibility_hash()
            .hash(&mut hasher);
        Some(hasher.finish().to_string())
    }
}

impl<T: std::clone::Clone + Sync + WasiConfig + Send + 'static> WasmtimeEngine<T> {
    /// Execute a wasm module.
    ///
    /// This function adds wasi_preview1 to the linker and can be utilized
    /// to execute a wasm module that uses wasi_preview1.
    fn execute_module(
        &self,
        module: Module,
        mut store: Store<WasiCtx>,
        func: &String,
    ) -> Result<std::prelude::v1::Result<(), anyhow::Error>, anyhow::Error> {
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
        component: Component,
        mut store: Store<WasiCtx>,
        func: String,
    ) -> Result<std::prelude::v1::Result<(), anyhow::Error>, anyhow::Error> {
        log::debug!("loading wasm component");

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

    fn execute(
        &self,
        wasm_binary: &[u8],
        store: Store<WasiCtx>,
        func: String,
    ) -> Result<std::prelude::v1::Result<(), anyhow::Error>, anyhow::Error> {
        match WasmBinaryType::from_bytes(wasm_binary) {
            Some(WasmBinaryType::Module) => {
                log::debug!("loading wasm module");
                let module = Module::from_binary(&self.engine, wasm_binary)?;
                self.execute_module(module, store, &func)
            }
            Some(WasmBinaryType::Component) => {
                let component = Component::from_binary(&self.engine, wasm_binary)?;
                self.execute_component(component, store, func)
            }
            None => match &self.engine.detect_precompiled(wasm_binary) {
                Some(Precompiled::Module) => {
                    log::info!("using precompiled module");
                    let module = unsafe { Module::deserialize(&self.engine, wasm_binary) }?;
                    self.execute_module(module, store, &func)
                }
                Some(Precompiled::Component) => {
                    log::info!("using precompiled component");
                    let component = unsafe { Component::deserialize(&self.engine, wasm_binary) }?;
                    self.execute_component(component, store, func)
                }
                None => {
                    bail!("invalid precompiled module")
                }
            },
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
        resource_table: ResourceTable::default(),
    };
    Ok(wasi_data)
}
