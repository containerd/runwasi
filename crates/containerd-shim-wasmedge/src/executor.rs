use std::path::PathBuf;

use anyhow::Result;
use containerd_shim_wasm::libcontainer_instance::LinuxContainerExecutor;
use containerd_shim_wasm::sandbox::{oci, Stdio};
use libcontainer::workload::{Executor, ExecutorError, ExecutorValidationError};
use log::debug;
use oci_spec::runtime::Spec;
use wasmedge_sdk::config::{CommonConfigOptions, ConfigBuilder, HostRegistrationConfigOptions};
use wasmedge_sdk::{params, VmBuilder};

const EXECUTOR_NAME: &str = "wasmedge";

#[derive(Clone)]
pub struct WasmEdgeExecutor {
    stdio: Stdio,
}

impl WasmEdgeExecutor {
    pub fn new(stdio: Stdio) -> Self {
        Self { stdio }
    }
}

impl Executor for WasmEdgeExecutor {
    fn exec(&self, spec: &Spec) -> Result<(), ExecutorError> {
        match can_handle(spec) {
            Ok(()) => {
                // parse wasi parameters
                let args = oci::get_args(spec);
                if args.is_empty() {
                    return Err(ExecutorError::InvalidArg);
                }

                let vm = self.prepare(args, spec).map_err(|err| {
                    ExecutorError::Other(format!("failed to prepare function: {}", err))
                })?;

                let (module_name, method) = oci::get_module(spec);
                debug!("running {:?} with method {}", module_name, method);
                if let Err(err) = vm.run_func(Some("main"), method, params!()) {
                    log::info!("failed to execute function: {err}");
                    std::process::exit(137);
                }

                let status = vm
                    .wasi_module()
                    .map(|module| module.exit_code())
                    .unwrap_or(137);
                std::process::exit(status as i32);
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
            Some(vec!["/:/"]),
        );

        let (module_name, _) = oci::get_module(spec);
        let module_name = match module_name {
            Some(m) => m,
            None => return Err(anyhow::Error::msg("no module provided cannot load module")),
        };
        let vm = vm
            .register_module_from_file("main", module_name)
            .map_err(|err| ExecutorError::Execution(err))?;

        self.stdio.take().redirect()?;

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

fn can_handle(spec: &Spec) -> Result<(), ExecutorValidationError> {
    // check if the entrypoint of the spec is a wasm binary.
    let (module_name, _method) = oci::get_module(spec);
    let module_name = match module_name {
        Some(m) => m,
        None => {
            log::info!("WasmEdge cannot handle this workload, no arguments provided");
            return Err(ExecutorValidationError::CantHandle(EXECUTOR_NAME));
        }
    };
    let path = PathBuf::from(module_name);

    path.extension()
        .map(|ext| ext.to_ascii_lowercase())
        .is_some_and(|ext| ext == "wasm" || ext == "wat")
        .then_some(())
        .ok_or(ExecutorValidationError::CantHandle(EXECUTOR_NAME))?;

    Ok(())
}
