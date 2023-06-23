use std::{fs::OpenOptions, path::PathBuf};

use anyhow::{anyhow, Result};
use containerd_shim_wasm::sandbox::oci;

use libcontainer::workload::{Executor, ExecutorError};
use oci_spec::runtime::Spec;

use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::WasiCtxBuilder;

use crate::oci_wasmtime::{self, wasi_dir, wasi_file};

const EXECUTOR_NAME: &str = "wasmtime";

static mut STDIN_FD: Option<RawFd> = None;
static mut STDOUT_FD: Option<RawFd> = None;
static mut STDERR_FD: Option<RawFd> = None;

pub struct WasmtimeExecutor {
    pub stdin: Option<RawFd>,
    pub stdout: Option<RawFd>,
    pub stderr: Option<RawFd>,
    pub engine: Engine,
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

    fn can_handle(&self, _spec: &Spec) -> bool {
        true
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
        let mut wasi_builder = WasiCtxBuilder::new()
            .args(args)?
            .envs(env.as_slice())?
            .preopened_dir(path, "/")?;

        if let Some(stdin) = self.stdin {
            unsafe {
                STDIN_FD = Some(dup(STDIN_FILENO));
                dup2(stdin, STDIN_FILENO);
            }
        }
        if let Some(stdout) = self.stdout {
            unsafe {
                STDOUT_FD = Some(dup(STDOUT_FILENO));
                dup2(stdout, STDOUT_FILENO);
            }
        }
        if let Some(stderr) = self.stderr {
            unsafe {
                STDERR_FD = Some(dup(STDERR_FILENO));
                dup2(stderr, STDERR_FILENO);
            }
        }
        log::info!("opening stdin");
        let stdin_path = PathBuf::from("/dev/stdin");
        let stdin_wasi_file = wasi_file(stdin_path, OpenOptions::new().read(true))?;
        wasi_builder = wasi_builder.stdin(Box::new(stdin_wasi_file));

        log::info!("opening stdout");
        let stdout_path = PathBuf::from("/dev/stdout");
        let stdout_wasi_file = wasi_file(stdout_path, OpenOptions::new().write(true))?;
        wasi_builder = wasi_builder.stdout(Box::new(stdout_wasi_file));

        log::info!("opening stderr");
        let stderr_path = PathBuf::from("/dev/stderr");
        let stderr_wasi_file = wasi_file(stderr_path, OpenOptions::new().write(true))?;
        wasi_builder = wasi_builder.stderr(Box::new(stderr_wasi_file));

        log::info!("building wasi context");
        let wctx = wasi_builder.build();

        log::info!("wasi context ready");
        let start = args[0].clone();
        let mut iterator = start.split('#');
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
        let i = linker.instantiate(&mut store, &module)?;

        log::info!("getting start function");
        let f = i
            .get_func(&mut store, method)
            .ok_or_else(|| anyhow!("module does not have a wasi start function".to_string()))?;
        Ok((store, f))
    }
}
