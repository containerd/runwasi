use anyhow::Result;
use containerd_shim_wasm::sandbox::oci;
use nix::unistd::{dup, dup2};
use oci_spec::runtime::Spec;

use libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use libcontainer::workload::{Executor, ExecutorError};
use log::debug;
use std::{os::unix::io::RawFd, path::PathBuf};

use wasmedge_sdk::{
    config::{CommonConfigOptions, ConfigBuilder, HostRegistrationConfigOptions},
    params, VmBuilder,
};

const EXECUTOR_NAME: &str = "wasmedge";

pub struct WasmEdgeExecutor {
    stdin: Option<RawFd>,
    stdout: Option<RawFd>,
    stderr: Option<RawFd>,
}

impl WasmEdgeExecutor {
    pub fn new(stdin: Option<RawFd>, stdout: Option<RawFd>, stderr: Option<RawFd>) -> Self {
        Self {
            stdin,
            stdout,
            stderr,
        }
    }
}

impl Executor for WasmEdgeExecutor {
    fn exec(&self, spec: &Spec) -> Result<(), ExecutorError> {
        // parse wasi parameters
        let args = oci::get_args(spec);
        if args.is_empty() {
            return Err(ExecutorError::InvalidArg);
        }

        let vm = self
            .prepare(args, spec)
            .map_err(|err| ExecutorError::Other(format!("failed to prepare function: {}", err)))?;

        // TODO: How to get exit code?
        // This was relatively straight forward in go, but wasi and wasmtime are totally separate things in rust
        let (module_name, method) = oci::get_module(spec);
        debug!("running {:?} with method {}", module_name, method);
        match vm.run_func(Some("main"), method, params!()) {
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
                log::info!("WasmEdge cannot handle this workload, no arguments provided");
                return false;
            }
        };
        let path = PathBuf::from(module_name);

        path.extension()
            .map(|ext| ext.to_ascii_lowercase())
            .is_some_and(|ext| ext == "wasm" || ext == "wat")
    }

    fn name(&self) -> &'static str {
        EXECUTOR_NAME
    }
}

impl WasmEdgeExecutor {
    fn prepare(&self, args: &[String], spec: &Spec) -> anyhow::Result<wasmedge_sdk::Vm> {
        let envs = env_to_wasi(spec);
        let config = ConfigBuilder::new(CommonConfigOptions::default())
            .with_host_registration_config(HostRegistrationConfigOptions::default().wasi(true))
            .build()
            .map_err(|err| ExecutorError::Execution(err))?;
        let mut vm = VmBuilder::new()
            .with_config(config)
            .build()
            .map_err(|err| ExecutorError::Execution(err))?;
        let wasi_module = vm
            .wasi_module_mut()
            .ok_or_else(|| anyhow::Error::msg("Not found wasi module"))
            .map_err(|err| ExecutorError::Execution(err.into()))?;
        wasi_module.initialize(
            Some(args.iter().map(|s| s as &str).collect()),
            Some(envs.iter().map(|s| s as &str).collect()),
            None,
        );

        let (module_name, _) = oci::get_module(spec);
        let module_name = match module_name {
            Some(m) => m,
            None => return Err(anyhow::Error::msg("no module provided cannot load module")),
        };
        let vm = vm
            .register_module_from_file("main", module_name)
            .map_err(|err| ExecutorError::Execution(err))?;
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
        Ok(vm)
    }
}

fn env_to_wasi(spec: &Spec) -> Vec<String> {
    let default = vec![];
    let env = spec
        .process()
        .as_ref()
        .unwrap()
        .env()
        .as_ref()
        .unwrap_or(&default);
    env.to_vec()
}
