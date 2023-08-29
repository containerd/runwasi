use nix::unistd::{dup, dup2};
use std::{fs::OpenOptions, os::fd::RawFd};

use anyhow::{anyhow, Context, Result};
use containerd_shim_wasm::sandbox::oci;
use libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use libcontainer::oci_spec::runtime::Spec;
use libcontainer::workload::{Executor, ExecutorError, ExecutorValidationError};

use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::WasiCtxBuilder;

use crate::oci_wasmtime::{self, wasi_dir};

#[derive(Clone)]
pub struct WasmtimeExecutor {
    pub stdin: Option<RawFd>,
    pub stdout: Option<RawFd>,
    pub stderr: Option<RawFd>,
    pub engine: Engine,
}

impl Executor for WasmtimeExecutor {
    fn exec(&self, spec: &Spec) -> Result<(), ExecutorError> {
        let args = oci::get_args(spec);
        if args.len() != 1 {
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

    fn validate(
        &self,
        spec: &Spec,
    ) -> std::result::Result<(), libcontainer::workload::ExecutorValidationError> {
        let args = oci::get_args(spec);
        if args.len() != 1 {
            return Err(ExecutorValidationError::InvalidArg);
        }

        Ok(())
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
        let mut iterator = args
            .first()
            .context("args must have at least one argument.")?
            .split('#');
        let mut cmd = iterator.next().unwrap().to_string();
        let stripped = cmd.strip_prefix(std::path::MAIN_SEPARATOR);
        if let Some(strpd) = stripped {
            cmd = strpd.to_string();
        }
        let method = iterator.next().unwrap_or("_start");
        let mod_path = cmd;

        log::info!("loading module from file");
        let module = Module::from_file(&self.engine, mod_path)?;
        let mut linker = Linker::new(&self.engine);

        wasmtime_wasi::add_to_linker(&mut linker, |s| s)?;
        let mut store = Store::new(&self.engine, wctx);

        log::info!("instantiating instance");
        let instance = linker.instantiate(&mut store, &module)?;

        log::info!("getting start function");
        let start_func = instance
            .get_func(&mut store, method)
            .ok_or_else(|| anyhow!("module does not have a WASI start function".to_string()))?;
        Ok((store, start_func))
    }
}
