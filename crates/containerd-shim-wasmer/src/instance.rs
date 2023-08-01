use anyhow::{bail, Context, Result};
use containerd_shim_wasm::container::{
    Engine, Instance, PathResolve, RuntimeContext, Stdio, WasiEntrypoint,
};
use wasmer::{Module, Store};
use wasmer_wasix::virtual_fs::host_fs::FileSystem;
use wasmer_wasix::{WasiEnv, WasiError};

pub type WasmerInstance = Instance<WasmerEngine>;

#[derive(Clone, Default)]
pub struct WasmerEngine {
    engine: wasmer::Cranelift,
}

impl Engine for WasmerEngine {
    fn name() -> &'static str {
        "wasmer"
    }

    fn run_wasi(&self, ctx: &impl RuntimeContext, stdio: Stdio) -> Result<i32> {
        let args = ctx.args();
        let envs = std::env::vars();
        let WasiEntrypoint { path, func } = ctx.wasi_entrypoint();

        let mod_name = match path.file_stem() {
            Some(name) => name.to_string_lossy().to_string(),
            None => "main".to_string(),
        };

        log::info!("Create a Store");
        let mut store = Store::new(self.engine.clone());

        let module = match ctx.oci_artifacts() {
            [] => {
                log::info!("loading module from file {path:?}");
                let path = path
                    .resolve_in_path_or_cwd()
                    .next()
                    .context("module not found")?;

                Module::from_file(&store, path)?
            }
            [module] => {
                log::info!("loading module from OCI Artifact");
                Module::from_binary(&store, &module.layer)?
            }
            [..] => bail!("only a single module is supported when using OCI Artifact"),
        };

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let _guard = runtime.enter();

        log::info!("Creating `WasiEnv`...: args {args:?}, envs: {envs:?}");
        let (instance, wasi_env) = WasiEnv::builder(mod_name)
            .args(&args[1..])
            .envs(envs)
            .fs(Box::<FileSystem>::default())
            .preopen_dir("/")?
            .instantiate(module, &mut store)?;

        log::info!("redirect stdio");
        stdio.redirect()?;

        log::info!("Running {func:?}");
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
