use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use anyhow::{bail, Context, Result};
use containerd_shim_wasm::container::{
    Engine, Entrypoint, Instance, RuntimeContext, Stdio, WasmBinaryType,
};
use containerd_shim_wasm::sandbox::WasmLayer;
use wasmtime::component::{self as wasmtime_component, Component, ResourceTable};
use wasmtime::{Config, Module, Precompiled, Store};
use wasmtime_wasi::preview1::{self as wasi_preview1};
use wasmtime_wasi::{self as wasi_preview2};

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
        let mut config = T::new_config();
        config.async_support(true); // must be on
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
    pub(crate) wasi_preview1: wasi_preview1::WasiP1Ctx,
    pub(crate) resource_table: ResourceTable,
}

/// This impl is required to use wasmtime_wasi::preview2::WasiView trait.
impl wasi_preview2::WasiView for WasiCtx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.resource_table
    }

    fn ctx(&mut self) -> &mut wasi_preview2::WasiCtx {
        &mut self.wasi_preview2
    }
}

impl<T: WasiConfig> Engine for WasmtimeEngine<T> {
    fn name() -> &'static str {
        "wasmtime"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
        log::info!("setting up wasi");
        let envs = ctx
            .envs()
            .iter()
            .map(|v| match v.split_once('=') {
                None => (v.to_string(), String::new()),
                Some((key, value)) => (key.to_string(), value.to_string()),
            })
            .collect::<Vec<_>>();

        let Entrypoint {
            source,
            func,
            arg0: _,
            name: _,
        } = ctx.entrypoint();

        log::info!("building wasi context");
        let wasi_ctx = prepare_wasi_ctx(ctx, &envs)?;
        let store = Store::new(&self.engine, wasi_ctx);

        let wasm_bytes = &source.as_bytes()?;

        let status = self.execute(wasm_bytes, store, func, stdio)?;

        let status = status.map(|_| 0).or_else(|err| {
            match err.downcast_ref::<wasmtime_wasi::I32Exit>() {
                Some(value) => Ok(value.process_exit_code()),
                _ => Err(err),
            }
        })?;

        Ok(status)
    }

    fn precompile(&self, layers: &[WasmLayer]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut compiled_layers = Vec::<Option<Vec<u8>>>::with_capacity(layers.len());

        for layer in layers {
            if self.engine.detect_precompiled(&layer.layer).is_some() {
                log::info!("Already precompiled");
                compiled_layers.push(None);
                continue;
            }

            let compiled_layer = self.engine.precompile_module(&layer.layer)?;
            compiled_layers.push(Some(compiled_layer));
        }

        Ok(compiled_layers)
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
        stdio: Stdio,
    ) -> Result<std::prelude::v1::Result<(), anyhow::Error>, anyhow::Error> {
        log::debug!("execute module");

        let mut module_linker = wasmtime::Linker::new(&self.engine);

        log::debug!("init linker");
        wasi_preview1::add_to_linker_async(&mut module_linker, |s: &mut WasiCtx| {
            &mut s.wasi_preview1
        })?;

        wasmtime_wasi::runtime::in_tokio(async move {
            log::info!("instantiating instance");
            let instance: wasmtime::Instance =
                module_linker.instantiate_async(&mut store, &module).await?;

            log::info!("getting start function");
            let start_func = instance
                .get_func(&mut store, func)
                .context("module does not have a WASI start function")?;

            log::debug!("running start function {func:?}");

            stdio.redirect()?;

            let status = start_func.call_async(&mut store, &[], &mut []).await;
            Ok(status)
        })
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
        stdio: Stdio,
    ) -> Result<std::prelude::v1::Result<(), anyhow::Error>, anyhow::Error> {
        log::debug!("loading wasm component");

        let mut linker = wasmtime_component::Linker::new(&self.engine);

        log::debug!("init linker");
        wasi_preview2::add_to_linker_async(&mut linker)?;
        log::debug!("done init linker");

        log::info!("instantiating component");

        // This is a adapter logic that converts wasip1 `_start` function to wasip2 `run` function.
        //
        // TODO: think about a better way to do this.
        wasmtime_wasi::runtime::in_tokio(async move {
            if func == "_start" {
                let pre = linker.instantiate_pre(&component)?;
                let (command, _instance) =
                    wasi_preview2::bindings::Command::instantiate_pre(&mut store, &pre).await?;

                stdio.redirect()?;

                let status = command
                    .wasi_cli_run()
                    .call_run(&mut store)
                    .await?
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "failed to run component targeting `wasi:cli/command` world"
                        )
                    });

                Ok(status)
            } else {
                let pre = linker.instantiate_pre(&component)?;

                let instance = pre.instantiate_async(&mut store).await?;

                log::info!("getting component exported function {func:?}");
                let start_func = instance.get_func(&mut store, &func).context(format!(
                    "component does not have exported function {func:?}"
                ))?;

                log::debug!("running exported function {func:?} {start_func:?}");

                stdio.redirect()?;

                let status = start_func.call_async(&mut store, &[], &mut []).await;
                Ok(status)
            }
        })
    }

    fn execute(
        &self,
        wasm_binary: &[u8],
        store: Store<WasiCtx>,
        func: String,
        stdio: Stdio,
    ) -> Result<std::prelude::v1::Result<(), anyhow::Error>, anyhow::Error> {
        match WasmBinaryType::from_bytes(wasm_binary) {
            Some(WasmBinaryType::Module) => {
                log::debug!("loading wasm module");
                let module = Module::from_binary(&self.engine, wasm_binary)?;
                self.execute_module(module, store, &func, stdio)
            }
            Some(WasmBinaryType::Component) => {
                let component = Component::from_binary(&self.engine, wasm_binary)?;
                self.execute_component(component, store, func, stdio)
            }
            None => match &self.engine.detect_precompiled(wasm_binary) {
                Some(Precompiled::Module) => {
                    log::info!("using precompiled module");
                    let module = unsafe { Module::deserialize(&self.engine, wasm_binary) }?;
                    self.execute_module(module, store, &func, stdio)
                }
                Some(Precompiled::Component) => {
                    log::info!("using precompiled component");
                    let component = unsafe { Component::deserialize(&self.engine, wasm_binary) }?;
                    self.execute_component(component, store, func, stdio)
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
    envs: &[(String, String)],
) -> Result<WasiCtx, anyhow::Error> {
    let mut wasi_preview1_builder = wasi_builder(ctx, envs)?;
    let wasi_preview1_ctx = wasi_preview1_builder.build_p1();

    let mut wasi_preview2_builder = wasi_builder(ctx, envs)?;
    let wasi_preview2_ctx = wasi_preview2_builder.build();
    let wasi_data = WasiCtx {
        wasi_preview1: wasi_preview1_ctx,
        wasi_preview2: wasi_preview2_ctx,
        resource_table: ResourceTable::default(),
    };
    Ok(wasi_data)
}

fn wasi_builder(
    ctx: &impl RuntimeContext,
    envs: &[(String, String)],
) -> Result<wasi_preview2::WasiCtxBuilder, anyhow::Error> {
    // TODO: make this more configurable (e.g. allow the user to specify the
    // preopened directories and their permissions)
    // https://github.com/containerd/runwasi/issues/413
    let file_perms = wasi_preview2::FilePerms::all();
    let dir_perms = wasi_preview2::DirPerms::all();

    let mut builder = wasi_preview2::WasiCtxBuilder::new();
    builder
        .args(ctx.args())
        .envs(envs)
        .inherit_stdio()
        .inherit_network()
        .allow_tcp(true)
        .allow_udp(true)
        .allow_ip_name_lookup(true)
        .preopened_dir("/", "/", dir_perms, file_perms)?;
    Ok(builder)
}
