use anyhow::{Context, Result};
use containerd_shim_wasm::container::{
    Engine, Entrypoint, Instance, RuntimeContext, Stdio
};
use wamr_rust_sdk::function::Function;
use wamr_rust_sdk::instance::Instance as WamrInstnace;
use wamr_rust_sdk::module::Module;
use wamr_rust_sdk::runtime::Runtime;

pub type WamrInstance = Instance<WamrEngine>;

pub struct WamrEngine {
    runtime: Runtime,
}

unsafe impl Send for WamrEngine {}
unsafe impl Sync for WamrEngine {}

// TODO: wasmr_rust_sdk::runtime::Runtime should implement Clone

impl Default for WamrEngine {
    fn default() -> Self {
        let runtime = Runtime::new().unwrap();
        Self { runtime }
    }
}

impl Clone for WamrEngine {
    fn clone(&self) -> Self {
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

        log::info!("Create a WamrInstance");

        // TODO: error handling isn't ideal

        let mut module = Module::from_buf(&wasm_bytes).map_err(|e| {
            anyhow::Error::msg(format!("Failed to create module from bytes: {:?}", e))
        })?;

        module.set_wasi_arg_pre_open_path(vec![String::from("/")], vec![String::from("/")]);
        module.set_wasi_arg_env_vars(envs);

        // TODO: no way to set args in wamr?
        // TODO: no way to register a named module with bytes?

        let instance = WamrInstnace::new(&module, 1024 * 64)
            .map_err(|e| anyhow::Error::msg(format!("Failed to create instance: {:?}", e)))?;

        // TODO: bug: failed at line above saying: `thread signal env initialized failed`

        log::info!("redirect stdio");
        stdio.redirect()?;

        log::info!("Running {func:?}");
        let function = Function::find_export_func(&instance, &func)
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
