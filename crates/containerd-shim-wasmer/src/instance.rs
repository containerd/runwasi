use anyhow::Result;
use containerd_shim_wasm::container::{Engine, Entrypoint, RuntimeContext};
use tokio::runtime::Handle;
use wasmer::{Module, Store};
use wasmer_wasix::virtual_fs::host_fs::FileSystem;
use wasmer_wasix::{WasiEnv, WasiError};

#[derive(Clone, Default)]
pub struct WasmerEngine {
    engine: wasmer::Cranelift,
}

impl Engine for WasmerEngine {
    fn name() -> &'static str {
        "wasmer"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32> {
        let args = ctx.args();
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
            name,
        } = ctx.entrypoint();

        let mod_name = name.unwrap_or_else(|| "main".to_string());

        containerd_shim_wasm::info!(ctx, "Create a Store");
        let mut store = Store::new(self.engine.clone());

        let wasm_bytes = source.as_bytes()?;
        let module = Module::from_binary(&store, &wasm_bytes)?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let _guard = runtime.enter();

        containerd_shim_wasm::info!(ctx, "Creating `WasiEnv`...: args {args:?}, envs: {envs:?}");
        let fs = FileSystem::new(Handle::current(), "/")?;
        let (instance, wasi_env) = WasiEnv::builder(mod_name)
            .args(&args[1..])
            .envs(envs)
            .fs(Box::new(fs))
            .preopen_dir("/")?
            .instantiate(module, &mut store)?;

        containerd_shim_wasm::info!(ctx, "Running {func:?}");
        let start = instance.exports.get_function(&func)?;
        wasi_env.data(&store).thread.set_status_running();
        let status = start.call(&mut store, &[]).map(|_| 0).or_else(|err| {
            match err.downcast_ref::<WasiError>() {
                Some(WasiError::Exit(code)) => Ok(code.raw()),
                _ => Err(err),
            }
        })?;

        Ok(status)
    }
}
