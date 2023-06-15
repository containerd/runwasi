use anyhow::Result;
use containerd_shim_wasm::sandbox::oci;
use nix::unistd::{dup, dup2};
use oci_spec::runtime::Spec;

use libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use libcontainer::workload::{Executor, ExecutorError};
use log::debug;
use std::os::unix::io::RawFd;
use wasmedge_sdk::{
    config::{CommonConfigOptions, ConfigBuilder, HostRegistrationConfigOptions},
    params, VmBuilder,
};

const EXECUTOR_NAME: &str = "wasmedge";

pub struct WasmEdgeExecutor {
    pub stdin: Option<RawFd>,
    pub stdout: Option<RawFd>,
    pub stderr: Option<RawFd>,
}

impl Executor for WasmEdgeExecutor {
    fn exec(&self, spec: &Spec) -> Result<(), ExecutorError> {
        let envs = env_to_wasi(spec);

        // create configuration with `wasi` option enabled
        let config = ConfigBuilder::new(CommonConfigOptions::default())
            .with_host_registration_config(HostRegistrationConfigOptions::default().wasi(true))
            .build()
            .map_err(|err| ExecutorError::Execution(err))?;

        // create a vm with the config settings
        let mut vm = VmBuilder::new()
            .with_config(config)
            .build()
            .map_err(|err| ExecutorError::Execution(err))?;

        // initialize the wasi module with the parsed parameters
        let wasi_module = vm
            .wasi_module_mut()
            .ok_or_else(|| anyhow::Error::msg("Not found wasi module"))
            .map_err(|err| ExecutorError::Execution(err.into()))?;

        let args = oci::get_module_args(spec);
        let mut module_args = None;
        if !args.is_empty() {
            module_args = Some(args.iter().map(|s| s as &str).collect())
        }

        debug!("module args: {:?}", module_args);
        wasi_module.initialize(
            module_args,
            Some(envs.iter().map(|s| s as &str).collect()),
            None,
        );

        let (module_name, method) = oci::get_module(spec);
        let module_name = match module_name {
            Some(m) => m,
            None => {
                return Err(ExecutorError::Execution(
                    anyhow::Error::msg(
                        "no module provided, cannot load module from file within container",
                    )
                    .into(),
                ))
            }
        };

        let vm = vm
            .register_module_from_file("main", module_name.clone())
            .map_err(|err| ExecutorError::Execution(err))?;

        if let Some(stdin) = self.stdin {
            let _ = dup(STDIN_FILENO);
            let _ = dup2(stdin, STDIN_FILENO);
        }
        if let Some(stdout) = self.stdout {
            let _ = dup(STDOUT_FILENO);
            let _ = dup2(stdout, STDOUT_FILENO);
        }
        if let Some(stderr) = self.stderr {
            let _ = dup(STDERR_FILENO);
            let _ = dup2(stderr, STDERR_FILENO);
        }

        debug!("running {:?} with method {}", module_name, method);
        match vm.run_func(Some("main"), method, params!()) {
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
