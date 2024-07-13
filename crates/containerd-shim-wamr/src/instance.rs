use anyhow::{Context, Result};
use containerd_shim_wasm::container::{
    Engine, Entrypoint, Instance, RuntimeContext, Stdio
};
use wamr_rust_sdk::function::Function;
use wamr_rust_sdk::instance::Instance as WamrInstnace;
use wamr_rust_sdk::module::Module;
use wamr_rust_sdk::runtime::Runtime;

// use wamr_rust_sdk::value::WasmValue;
use wamr_rust_sdk::wasi_context::WasiCtxBuilder;
// use wamr_rust_sdk::RuntimeError;

pub type WamrInstance = Instance<WamrEngine>;

pub struct WamrEngine {
    runtime: Runtime,
}

unsafe impl Send for WamrEngine {}
unsafe impl Sync for WamrEngine {}

// TODO: wasmr_rust_sdk::runtime::Runtime should implement Clone

impl Default for WamrEngine {
    fn default() -> Self {
        log::info!("Create a WAMR runtime");
        let runtime = Runtime::new().unwrap();
        Self { runtime }
    }
}

impl Clone for WamrEngine {
    fn clone(&self) -> Self {
        log::info!("Clone a WAMR runtime");
        let runtime = Runtime::new().unwrap();
        Self { runtime }
    }
}

impl Engine for WamrEngine {
    fn name() -> &'static str {
        "wamr"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
        let args = ctx.args();
        let envs: Vec<_> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
        let Entrypoint {
            source,
            func,
            arg0: _,
            name,
        } = ctx.entrypoint();

        let wasm_bytes = source
            .as_bytes()
            .context("Failed to get bytes from source")?;

        // log the source content
        log::info!("Wasm source: {source:?}");
        //and bytes
        // log::info!("Wasm bytes: {wasm_bytes:?}");

        log::info!("Create a WAMR module");

        // TODO: error handling isn't ideal

        let mut module = Module::from_buf(&self.runtime, &wasm_bytes, "main").map_err(|e| {
            anyhow::Error::msg(format!("Failed to create module from bytes: {:?}", e))
        })?;

        log::info!("Create a WAMR vec_of_strs");

        //log envs
        log::info!("envs: {envs:?}");

        let vec_of_strs: Vec<&str> = envs.iter().map(|s| s.as_str()).collect();

        log::info!("Create a WAMR wasi_ctx");

        let wasi_ctx = WasiCtxBuilder::new()
        // .set_pre_open_path(vec!["/"], vec!["/"])
        // .set_env_vars(vec_of_strs)
        .build();

        log::info!("Create a WAMR set_wasi_context");

        module.set_wasi_context(wasi_ctx);
        // module.set_wasi_arg_pre_open_path(vec![String::from("/")], vec![String::from("/")]);
        // module.set_wasi_arg_env_vars(envs);

        // TODO: no way to set args in wamr?
        // TODO: no way to register a named module with bytes?

        log::info!("Create a WAMR instance");

        let instance = WamrInstnace::new(&self.runtime, &module, 1024 * 64)
            .map_err(|e| anyhow::Error::msg(format!("Failed to create instance: {:?}", e)))?;

        // TODO: bug: failed at line above saying: `thread signal env initialized failed`

        log::info!("redirect stdio");
        stdio.redirect()?;

        log::info!("Running {func:?}");
        let function = Function::find_export_func(&instance, "main")
            .map_err(|e| anyhow::Error::msg(format!("Failed to find function: {:?}", e)))?;
        let status = function
            .call(&instance, &Vec::new())
            .map(|_| 0)
            .or_else(|err| {
                log::error!("Error: {:?}", err);
                Err(err)
            })
            .map_err(|e| anyhow::Error::msg(format!("Failed to call function: {:?}", e)))?;

        Ok(status)
    }
}