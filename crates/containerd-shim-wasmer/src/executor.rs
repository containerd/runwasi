use std::path::PathBuf;

use containerd_shim_wasm::libcontainer_instance::LinuxContainerExecutor;
use containerd_shim_wasm::sandbox::oci::{self, Spec};
use containerd_shim_wasm::sandbox::Stdio;
use libcontainer::workload::{Executor, ExecutorError, ExecutorValidationError};
use wasmer::{Cranelift, Module, Store};
use wasmer_wasix::{WasiEnv, WasiError};

const EXECUTOR_NAME: &str = "wasmer";

#[derive(Clone)]
pub struct WasmerExecutor {
    stdio: Stdio,
    engine: Cranelift,
}

impl WasmerExecutor {
    pub fn new(stdio: Stdio, engine: Cranelift) -> Self {
        Self { stdio, engine }
    }
}

impl Executor for WasmerExecutor {
    fn exec(&self, spec: &Spec) -> Result<(), ExecutorError> {
        match can_handle(spec) {
            Ok(()) => {
                let args = oci::get_args(spec);
                if args.is_empty() {
                    return Err(ExecutorError::InvalidArg);
                }

                let status = self
                    .start(spec, args)
                    .map_err(|err| ExecutorError::Other(format!("failed to prepare: {}", err)))?;

                std::process::exit(status)
            }
            Err(ExecutorValidationError::CantHandle(_)) => {
                LinuxContainerExecutor::new(self.stdio.take()).exec(spec)?;

                Ok(())
            }
            Err(_) => Err(ExecutorError::InvalidArg),
        }
    }

    fn validate(&self, spec: &Spec) -> std::result::Result<(), ExecutorValidationError> {
        match can_handle(spec) {
            Ok(()) => Ok(()),
            Err(ExecutorValidationError::CantHandle(_)) => {
                LinuxContainerExecutor::new(self.stdio.clone()).validate(spec)?;

                Ok(())
            }
            Err(err) => Err(err),
        }
    }
}

impl WasmerExecutor {
    fn start(&self, spec: &Spec, args: &[String]) -> anyhow::Result<i32> {
        log::info!("get envs from spec");
        let envs = std::env::vars();

        log::info!("redirect stdio");
        self.stdio.take().redirect()?;

        log::info!("get module_name and method");
        let (module_name, method) = oci::get_module(spec);
        let module_name = match module_name {
            Some(m) => m,
            None => {
                return Err(anyhow::format_err!(
                    "no module provided, cannot load module from file within container"
                ))
            }
        };

        log::info!("Create a Store");
        let mut store = Store::new(self.engine.clone());

        log::info!("loading module from file {} ", module_name);
        let module = Module::from_file(&store, module_name)?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let _guard = runtime.enter();

        log::info!("Creating `WasiEnv`...: args {:?}, envs: {:?}", args, envs);
        let (instance, wasi_env) = WasiEnv::builder(EXECUTOR_NAME)
            .args(&args[1..])
            .envs(envs)
            .preopen_dir("/")?
            .instantiate(module, &mut store)?;

        log::info!("Running {method:?}");
        let start = instance.exports.get_function(&method)?;
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

fn can_handle(spec: &Spec) -> Result<(), ExecutorValidationError> {
    // check if the entrypoint of the spec is a wasm binary.
    let (module_name, _method) = oci::get_module(spec);
    let module_name = match module_name {
        Some(m) => m,
        None => {
            log::info!("Wasmtime cannot handle this workload, no arguments provided");
            return Err(ExecutorValidationError::CantHandle(EXECUTOR_NAME));
        }
    };
    let path = PathBuf::from(module_name);

    // TODO: do we need to validate the wasm binary?
    // ```rust
    //   let bytes = std::fs::read(path).unwrap();
    //   wasmparser::validate(&bytes).is_ok()
    // ```

    path.extension()
        .map(|ext| ext.to_ascii_lowercase())
        .is_some_and(|ext| ext == "wasm" || ext == "wat")
        .then_some(())
        .ok_or(ExecutorValidationError::CantHandle(EXECUTOR_NAME))?;

    Ok(())
}
