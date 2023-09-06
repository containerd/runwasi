use std::fs::OpenOptions;
use std::path::PathBuf;

use anyhow::{Context, Result};
use containerd_shim_wasm::libcontainer_instance::LinuxContainerExecutor;
use containerd_shim_wasm::sandbox::{oci, Stdio};
use libcontainer::workload::{Executor, ExecutorError, ExecutorValidationError};
use oci_spec::runtime::Spec;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::{maybe_exit_on_error, WasiCtxBuilder};

use crate::oci_wasmtime::{self, wasi_dir};

const EXECUTOR_NAME: &str = "wasmtime";

#[derive(Clone)]
pub struct WasmtimeExecutor {
    stdio: Stdio,
    engine: Engine,
}

impl WasmtimeExecutor {
    pub fn new(stdio: Stdio, engine: Engine) -> Self {
        Self { stdio, engine }
    }
}

impl Executor for WasmtimeExecutor {
    fn exec(&self, spec: &Spec) -> Result<(), ExecutorError> {
        match can_handle(spec) {
            Ok(()) => {
                let args = oci::get_args(spec);
                if args.is_empty() {
                    return Err(ExecutorError::InvalidArg);
                }

                let (mut store, f) = self.prepare(spec, args).map_err(|err| {
                    ExecutorError::Other(format!("failed to prepare function: {}", err))
                })?;

                log::info!("calling start function");

                let status = f.call(&mut store, &[], &mut []);
                let status = status
                    .map(|_| 0)
                    .map_err(maybe_exit_on_error)
                    .unwrap_or(137);

                std::process::exit(status);
            }
            Err(ExecutorValidationError::CantHandle(_)) => {
                LinuxContainerExecutor::new(self.stdio.clone()).exec(spec)?;

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

impl WasmtimeExecutor {
    fn prepare(
        &self,
        spec: &Spec,
        args: &[String],
    ) -> anyhow::Result<(Store<wasi_common::WasiCtx>, wasmtime::Func)> {
        // already in the cgroup
        let env = oci_wasmtime::env_to_wasi(spec);
        log::info!("setting up wasi");

        let path = wasi_dir("/", OpenOptions::new().read(true))?;
        let wasi_builder = WasiCtxBuilder::new()
            .args(args)?
            .envs(env.as_slice())?
            .inherit_stdio()
            .preopened_dir(path, "/")?;

        self.stdio.take().redirect()?;

        log::info!("building wasi context");
        let wctx = wasi_builder.build();

        log::info!("wasi context ready");
        let (module_name, method) = oci::get_module(spec);
        let module_name = module_name
            .context("no module provided, cannot load module from file within container")?;

        log::info!("loading module from file {}", module_name);
        let module = Module::from_file(&self.engine, module_name)?;
        let mut linker = Linker::new(&self.engine);

        wasmtime_wasi::add_to_linker(&mut linker, |s| s)?;
        let mut store = Store::new(&self.engine, wctx);

        log::info!("instantiating instance");
        let instance = linker.instantiate(&mut store, &module)?;

        log::info!("getting start function");
        let start_func = instance
            .get_func(&mut store, &method)
            .context("module does not have a WASI start function")?;
        Ok((store, start_func))
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
