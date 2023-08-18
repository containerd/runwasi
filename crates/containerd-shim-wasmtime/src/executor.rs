use std::{fs::OpenOptions, os::fd::RawFd, path::PathBuf};

use anyhow::{anyhow, Result};
use containerd_shim_wasm::sandbox::oci;
use libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use libcontainer::workload::{Executor, ExecutorError};
use nix::unistd::{dup, dup2};
use oci_spec::runtime::Spec;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::WasiCtxBuilder;

use crate::oci_wasmtime::{self, wasi_dir};

const EXECUTOR_NAME: &str = "wasmtime";

pub struct WasmtimeExecutor {
    stdin: Option<RawFd>,
    stdout: Option<RawFd>,
    stderr: Option<RawFd>,
    engine: Engine,
}

impl WasmtimeExecutor {
    pub fn new(
        stdin: Option<RawFd>,
        stdout: Option<RawFd>,
        stderr: Option<RawFd>,
        engine: Engine,
    ) -> Self {
        Self {
            stdin,
            stdout,
            stderr,
            engine,
        }
    }
}

impl Executor for WasmtimeExecutor {
    fn exec(&self, spec: &Spec) -> Result<(), ExecutorError> {
        let args = oci::get_args(spec);
        if args.is_empty() {
            return Err(ExecutorError::InvalidArg);
        }

        let (mut store, f) = self
            .prepare(spec, args)
            .map_err(|err| ExecutorError::Other(format!("failed to prepare function: {}", err)))?;

        log::info!("calling start function");
        match f.call(&mut store, &[], &mut []) {
            Ok(_) => std::process::exit(0),
            Err(_) => std::process::exit(137),
        };
    }

    fn can_handle(&self, spec: &Spec) -> bool {
        // check if the entrypoint of the spec is a wasm binary.
        let (module_name, _method) = oci::get_module(spec);
        let module_name = match module_name {
            Some(m) => m,
            None => {
                log::info!("Wasmtime cannot handle this workload, no arguments provided");
                return false;
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
    }

    fn name(&self) -> &'static str {
        EXECUTOR_NAME
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

        let path = wasi_dir(".", OpenOptions::new().read(true))?;
        let wasi_builder = WasiCtxBuilder::new()
            .args(args)?
            .envs(env.as_slice())?
            .inherit_stdio()
            .preopened_dir(path, "/")?;

        if let Some(stdin) = self.stdin {
            dup(STDIN_FILENO)?;
            dup2(stdin, STDIN_FILENO)?;
        }
        if let Some(stdout) = self.stdout {
            dup(STDOUT_FILENO)?;
            dup2(stdout, STDOUT_FILENO)?;
        }
        if let Some(stderr) = self.stderr {
            dup(STDERR_FILENO)?;
            dup2(stderr, STDERR_FILENO)?;
        }

        log::info!("building wasi context");
        let wctx = wasi_builder.build();

        log::info!("wasi context ready");
        let (module_name, method) = oci::get_module(spec);
        let module_name = match module_name {
            Some(m) => m,
            None => {
                return Err(anyhow::format_err!(
                    "no module provided, cannot load module from file within container"
                ))
            }
        };

        log::info!("loading module from file {} ", module_name);
        let module = Module::from_file(&self.engine, module_name)?;
        let mut linker = Linker::new(&self.engine);

        wasmtime_wasi::add_to_linker(&mut linker, |s| s)?;
        let mut store = Store::new(&self.engine, wctx);

        log::info!("instantiating instance");
        let instance = linker.instantiate(&mut store, &module)?;

        log::info!("getting start function");
        let start_func = instance
            .get_func(&mut store, &method)
            .ok_or_else(|| anyhow!("module does not have a WASI start function".to_string()))?;
        Ok((store, start_func))
    }
}
