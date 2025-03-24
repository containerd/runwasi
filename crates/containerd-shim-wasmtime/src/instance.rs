use std::hash::Hash;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use containerd_shim_wasm::container::{
    Compiler, Entrypoint, RuntimeContext, Sandbox, Shim, WasmBinaryType,
};
use containerd_shim_wasm::sandbox::WasmLayer;
use tokio_util::sync::CancellationToken;
use wasi_preview1::WasiP1Ctx;
use wasi_preview2::bindings::Command;
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{self, Component, ResourceTable};
use wasmtime::{Config, Module, Precompiled, Store};
use wasmtime_wasi::preview1::{self as wasi_preview1};
use wasmtime_wasi::{self as wasi_preview2};
use wasmtime_wasi_http::bindings::ProxyPre;
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::http_proxy::serve_conn;

/// Represents the WASI API that the component is targeting.
enum ComponentTarget<'a> {
    /// A component that targets WASI command-line interface.
    Command,
    /// A component that targets WASI http/proxy  interface.
    HttpProxy,
    /// Core function. The `&'a str` represents function to call.
    Core(&'a str),
}

impl<'a> ComponentTarget<'a> {
    fn new<'b, I>(exports: I, func: &'a str) -> Self
    where
        I: IntoIterator<Item = (&'b str, ComponentItem)> + 'b,
    {
        // This is heuristic but seems to work
        exports
            .into_iter()
            .find_map(|(name, _)| {
                if name.starts_with("wasi:http/incoming-handler") {
                    Some(Self::HttpProxy)
                } else if name.starts_with("wasi:cli/run") {
                    Some(Self::Command)
                } else {
                    None
                }
            })
            .unwrap_or(Self::Core(func))
    }
}

#[derive(Clone, Default)]
pub struct WasmtimeShim;

pub struct WasmtimeCompiler(wasmtime::Engine);

pub struct WasmtimeSandbox {
    engine: wasmtime::Engine,
    cancel: CancellationToken,
}

impl Default for WasmtimeSandbox {
    fn default() -> Self {
        let mut config = wasmtime::Config::new();

        // Disable Wasmtime parallel compilation for the tests
        // see https://github.com/containerd/runwasi/pull/405#issuecomment-1928468714 for details
        config.parallel_compilation(!cfg!(test));
        config.wasm_component_model(true); // enable component linking
        config.async_support(true); // must be on

        if use_pooling_allocator_by_default() {
            let cfg = wasmtime::PoolingAllocationConfig::default();
            config.allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(cfg));
        }

        Self {
            engine: wasmtime::Engine::new(&config)
                .context("failed to create wasmtime engine")
                .unwrap(),
            cancel: CancellationToken::new(),
        }
    }
}

pub struct WasiPreview2Ctx {
    pub(crate) wasi_ctx: wasi_preview2::WasiCtx,
    pub(crate) wasi_http: WasiHttpCtx,
    pub(crate) resource_table: ResourceTable,
}

impl WasiPreview2Ctx {
    pub fn new(ctx: &impl RuntimeContext) -> Result<Self> {
        containerd_shim_wasm::debug!(ctx, "Creating new WasiPreview2Ctx");
        Ok(Self {
            wasi_ctx: wasi_builder(ctx)?.build(),
            wasi_http: WasiHttpCtx::new(),
            resource_table: ResourceTable::default(),
        })
    }
}

/// This impl is required to use wasmtime_wasi::preview2::WasiView trait.
impl wasi_preview2::WasiView for WasiPreview2Ctx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.resource_table
    }

    fn ctx(&mut self) -> &mut wasi_preview2::WasiCtx {
        &mut self.wasi_ctx
    }
}

impl WasiHttpView for WasiPreview2Ctx {
    fn table(&mut self) -> &mut wasmtime::component::ResourceTable {
        &mut self.resource_table
    }

    fn ctx(&mut self) -> &mut wasmtime_wasi_http::WasiHttpCtx {
        &mut self.wasi_http
    }
}

impl Shim for WasmtimeShim {
    fn name() -> &'static str {
        "wasmtime"
    }

    type Sandbox = WasmtimeSandbox;

    #[allow(refining_impl_trait)]
    async fn compiler() -> Option<WasmtimeCompiler> {
        let mut config = wasmtime::Config::new();

        // Disable Wasmtime parallel compilation for the tests
        // see https://github.com/containerd/runwasi/pull/405#issuecomment-1928468714 for details
        config.parallel_compilation(!cfg!(test));
        config.wasm_component_model(true); // enable component linking
        config.async_support(true); // must be on

        let engine = wasmtime::Engine::new(&config)
            .expect("failed to create wasmtime precompilation engine");

        Some(WasmtimeCompiler(engine))
    }
}

impl Sandbox for WasmtimeSandbox {
    async fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32> {
        containerd_shim_wasm::info!(ctx, "setting up wasi");

        let Entrypoint {
            source,
            func,
            arg0: _,
            name: _,
        } = ctx.entrypoint();

        let wasm_bytes = &source.as_bytes()?;

        self.execute(ctx, wasm_bytes, func).await.into_error_code()
    }
}

impl Compiler for WasmtimeCompiler {
    fn cache_key(&self) -> impl Hash {
        self.0.precompile_compatibility_hash()
    }

    async fn compile(&self, layers: &[WasmLayer]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut compiled_layers = Vec::<Option<Vec<u8>>>::with_capacity(layers.len());

        for layer in layers {
            if self.0.detect_precompiled(&layer.layer).is_some() {
                log::info!("Already precompiled");
                compiled_layers.push(None);
                continue;
            }

            use WasmBinaryType::*;

            let compiled_layer = match WasmBinaryType::from_bytes(&layer.layer) {
                Some(Module) => self.0.precompile_module(&layer.layer)?,
                Some(Component) => self.0.precompile_component(&layer.layer)?,
                None => {
                    log::warn!("Unknown WASM binary type");
                    continue;
                }
            };

            compiled_layers.push(Some(compiled_layer));
        }

        Ok(compiled_layers)
    }
}

impl WasmtimeSandbox {
    /// Execute a wasm module.
    ///
    /// This function adds wasi_preview1 to the linker and can be utilized
    /// to execute a wasm module that uses wasi_preview1.
    async fn execute_module(
        &self,
        ctx: &impl RuntimeContext,
        module: Module,
        func: &String,
    ) -> Result<i32> {
        containerd_shim_wasm::debug!(ctx, "execute module");

        let ctx_p1 = wasi_builder(ctx)?.build_p1();
        let mut store = Store::new(&self.engine, ctx_p1);
        let mut module_linker = wasmtime::Linker::new(&self.engine);

        containerd_shim_wasm::debug!(ctx, "init linker");
        wasi_preview1::add_to_linker_async(&mut module_linker, |wasi_ctx: &mut WasiP1Ctx| {
            wasi_ctx
        })?;

        containerd_shim_wasm::info!(ctx, "instantiating instance");
        let instance: wasmtime::Instance =
            module_linker.instantiate_async(&mut store, &module).await?;

        containerd_shim_wasm::debug!(ctx, "getting start function");
        let start_func = instance
            .get_func(&mut store, func)
            .context("module does not have a WASI start function")?;

        containerd_shim_wasm::info!(ctx, "running start function {func:?}");

        start_func
            .call_async(&mut store, &[], &mut [])
            .await
            .into_error_code()
    }

    async fn execute_component_async(
        &self,
        ctx: &impl RuntimeContext,
        component: Component,
        func: String,
    ) -> Result<i32> {
        containerd_shim_wasm::info!(ctx, "instantiating component");

        let target = ComponentTarget::new(
            component.component_type().exports(&self.engine),
            func.as_str(),
        );

        // This is a adapter logic that converts wasip1 `_start` function to wasip2 `run` function.
        let status = match target {
            ComponentTarget::HttpProxy => {
                containerd_shim_wasm::info!(ctx, "Found HTTP proxy target");
                let mut linker = component::Linker::new(&self.engine);
                wasmtime_wasi::add_to_linker_async(&mut linker)?;
                wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

                let pre = linker.instantiate_pre(&component)?;
                containerd_shim_wasm::info!(ctx, "pre-instantiate_pre");
                let instance = ProxyPre::new(pre)?;

                containerd_shim_wasm::info!(ctx, "starting HTTP server");
                let cancel = self.cancel.clone();
                serve_conn(ctx, instance, cancel).await
            }
            ComponentTarget::Command => {
                containerd_shim_wasm::info!(ctx, "Found command target");
                let wasi_ctx = WasiPreview2Ctx::new(ctx)?;
                let (mut store, linker) = store_for_context(&self.engine, wasi_ctx)?;

                let command = Command::instantiate_async(&mut store, &component, &linker).await?;

                command
                    .wasi_cli_run()
                    .call_run(&mut store)
                    .await?
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "failed to run component targeting `wasi:cli/command` world"
                        )
                    })
            }
            ComponentTarget::Core(func) => {
                containerd_shim_wasm::info!(ctx, "Found Core target");
                let wasi_ctx = WasiPreview2Ctx::new(ctx)?;
                let (mut store, linker) = store_for_context(&self.engine, wasi_ctx)?;

                let pre = linker.instantiate_pre(&component)?;
                let instance = pre.instantiate_async(&mut store).await?;

                containerd_shim_wasm::info!(ctx, "getting component exported function {func:?}");
                let start_func = instance.get_func(&mut store, func).context(format!(
                    "component does not have exported function {func:?}"
                ))?;

                containerd_shim_wasm::debug!(
                    ctx,
                    "running exported function {func:?} {start_func:?}"
                );
                start_func.call_async(&mut store, &[], &mut []).await
            }
        };

        status.into_error_code()
    }

    /// Execute a wasm component.
    ///
    /// This function adds wasi_preview2 to the linker and can be utilized
    /// to execute a wasm component that uses wasi_preview2.
    async fn execute_component(
        &self,
        ctx: &impl RuntimeContext,
        component: Component,
        func: String,
    ) -> Result<i32> {
        containerd_shim_wasm::debug!(ctx, "loading wasm component");
        tokio::select! {
            status = self.execute_component_async(ctx, component, func) => {
                status
            }
            status = self.handle_signals() => {
                status
            }
        }
    }

    async fn handle_signals(&self) -> Result<i32> {
        match wait_for_signal().await? {
            libc::SIGINT => {
                // Request graceful shutdown;
                self.cancel.cancel();
            }
            sig => {
                // On other signal, terminate the process without waiting for spawned tasks to finish.
                return Ok(128 + sig);
            }
        }

        // On a second SIGINT, terminate the process as well
        wait_for_signal().await
    }

    async fn execute(
        &self,
        ctx: &impl RuntimeContext,
        wasm_binary: &[u8],
        func: String,
    ) -> Result<i32> {
        match WasmBinaryType::from_bytes(wasm_binary) {
            Some(WasmBinaryType::Module) => {
                containerd_shim_wasm::debug!(ctx, "loading wasm module");
                let module = Module::from_binary(&self.engine, wasm_binary)?;
                self.execute_module(ctx, module, &func).await
            }
            Some(WasmBinaryType::Component) => {
                let component = Component::from_binary(&self.engine, wasm_binary)?;
                self.execute_component(ctx, component, func).await
            }
            None => match &self.engine.detect_precompiled(wasm_binary) {
                Some(Precompiled::Module) => {
                    containerd_shim_wasm::info!(ctx, "using precompiled module");
                    let module = unsafe { Module::deserialize(&self.engine, wasm_binary) }?;
                    self.execute_module(ctx, module, &func).await
                }
                Some(Precompiled::Component) => {
                    containerd_shim_wasm::info!(ctx, "using precompiled component");
                    let component = unsafe { Component::deserialize(&self.engine, wasm_binary) }?;
                    self.execute_component(ctx, component, func).await
                }
                None => {
                    bail!("invalid precompiled module")
                }
            },
        }
    }
}

pub(crate) fn envs_from_ctx(ctx: &impl RuntimeContext) -> Vec<(String, String)> {
    ctx.envs()
        .iter()
        .map(|v| {
            let (key, value) = v.split_once('=').unwrap_or((v.as_str(), ""));
            (key.to_string(), value.to_string())
        })
        .collect()
}

fn store_for_context<T: wasi_preview2::WasiView>(
    engine: &wasmtime::Engine,
    ctx: T,
) -> Result<(Store<T>, component::Linker<T>)> {
    let store = Store::new(engine, ctx);

    log::debug!("init linker");
    let mut linker = component::Linker::new(engine);
    wasi_preview2::add_to_linker_async(&mut linker)?;

    Ok((store, linker))
}

fn wasi_builder(ctx: &impl RuntimeContext) -> Result<wasi_preview2::WasiCtxBuilder, anyhow::Error> {
    // TODO: make this more configurable (e.g. allow the user to specify the
    // preopened directories and their permissions)
    // https://github.com/containerd/runwasi/issues/413
    containerd_shim_wasm::debug!(ctx, "building WASI context");

    let file_perms = wasi_preview2::FilePerms::all();
    let dir_perms = wasi_preview2::DirPerms::all();
    let envs = envs_from_ctx(ctx);

    let mut builder = wasi_preview2::WasiCtxBuilder::new();
    builder
        .args(ctx.args())
        .envs(&envs)
        .inherit_stdio()
        .inherit_network()
        .allow_tcp(true)
        .allow_udp(true)
        .allow_ip_name_lookup(true)
        .preopened_dir("/", "/", dir_perms, file_perms)?;

    containerd_shim_wasm::debug!(ctx, "WASI context built successfully");
    Ok(builder)
}

async fn wait_for_signal() -> Result<i32> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigquit = signal(SignalKind::quit())?;
        let mut sigterm = signal(SignalKind::terminate())?;

        tokio::select! {
            _ = sigquit.recv() => { Ok(libc::SIGQUIT) }
            _ = sigterm.recv() => { Ok(libc::SIGTERM) }
            _ = tokio::signal::ctrl_c() => { Ok(libc::SIGINT) }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await;
        Ok(1)
    }
}

/// The pooling allocator is tailor made for the `wasi/http` use case. Check if we can use it.
///
/// For more details refer to: <https://github.com/bytecodealliance/wasmtime/blob/v27.0.0/src/commands/serve.rs#L641>
fn use_pooling_allocator_by_default() -> bool {
    static SUPPORTS_POOLING_ALLOCATOR: LazyLock<bool> = LazyLock::new(|| {
        const BITS_TO_TEST: u32 = 42;
        let mut config = Config::new();
        config.wasm_memory64(true);
        config.static_memory_maximum_size(1 << BITS_TO_TEST);
        let Ok(engine) = wasmtime::Engine::new(&config) else {
            return false;
        };
        let mut store = Store::new(&engine, ());
        let ty = wasmtime::MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)));
        wasmtime::Memory::new(&mut store, ty).is_ok()
    });
    *SUPPORTS_POOLING_ALLOCATOR
}

pub trait IntoErrorCode {
    fn into_error_code(self) -> Result<i32>;
}

impl IntoErrorCode for Result<i32> {
    fn into_error_code(self) -> Result<i32> {
        self.or_else(|err| match err.downcast_ref::<wasmtime_wasi::I32Exit>() {
            Some(exit) => Ok(exit.0),
            _ => Err(err),
        })
    }
}

impl IntoErrorCode for Result<()> {
    fn into_error_code(self) -> Result<i32> {
        self.map(|_| 0).into_error_code()
    }
}
