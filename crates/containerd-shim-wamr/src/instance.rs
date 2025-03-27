use anyhow::{Context, Result};
use containerd_shim_wasm::sandbox::Sandbox;
use containerd_shim_wasm::sandbox::context::{Entrypoint, RuntimeContext};
use containerd_shim_wasm::shim::{Shim, Version, version};
use wamr_rust_sdk::function::Function;
use wamr_rust_sdk::instance::Instance as WamrInst;
use wamr_rust_sdk::module::Module;
use wamr_rust_sdk::runtime::Runtime;
use wamr_rust_sdk::wasi_context::WasiCtxBuilder;

pub struct WamrShim;

pub struct WamrSandbox {
    runtime: Runtime,
}

unsafe impl Send for WamrSandbox {}
unsafe impl Sync for WamrSandbox {}

impl Default for WamrSandbox {
    fn default() -> Self {
        let runtime = Runtime::new().unwrap();
        Self { runtime }
    }
}

impl Shim for WamrShim {
    type Sandbox = WamrSandbox;

    fn name() -> &'static str {
        "wamr"
    }

    fn version() -> Version {
        version!()
    }
}

impl Sandbox for WamrSandbox {
    async fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32> {
        let args = ctx.args();
        let envs = ctx.envs();
        let Entrypoint {
            source, func, name, ..
        } = ctx.entrypoint();

        let wasm_bytes = source
            .as_bytes()
            .context("Failed to get bytes from source")?;

        log::info!("Create a WAMR module");

        // TODO: error handling isn't ideal

        let mod_name = name.unwrap_or_else(|| "main".to_string());

        let mut module = Module::from_buf(&self.runtime, &wasm_bytes, &mod_name)
            .context("Failed to create module from bytes")?;

        log::info!("Create a WASI context");

        let wasi_ctx = WasiCtxBuilder::new()
            .set_pre_open_path(vec!["/"], vec![])
            .set_env_vars(envs.iter().map(String::as_str).collect())
            .set_arguments(args.iter().map(String::as_str).collect())
            .build();

        module.set_wasi_context(wasi_ctx);

        // TODO: no way to register a named module with bytes?

        log::info!("Create a WAMR instance");

        let instance = WamrInst::new(&self.runtime, &module, 1024 * 64)
            .context("Failed to create instance")?;

        log::info!("Running {func:?}");
        let function =
            Function::find_export_func(&instance, &func).context("Failed to find function")?;
        let status = function
            .call(&instance, &vec![])
            .map(|_| 0)
            .map_err(|err| {
                log::error!("Error: {err:?}");
                err
            })
            .context("Failed to call function")?;

        Ok(status)
    }
}
