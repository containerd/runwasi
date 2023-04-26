use anyhow::{bail, Result};
use oci_spec::runtime::Spec;

use libc::{dup, dup2, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use libcontainer::workload::Executor;
use std::os::unix::io::RawFd;

use wasmedge_sdk::{
    config::{CommonConfigOptions, ConfigBuilder, HostRegistrationConfigOptions},
    params, Vm,
};

static mut STDIN_FD: Option<RawFd> = None;
static mut STDOUT_FD: Option<RawFd> = None;
static mut STDERR_FD: Option<RawFd> = None;

const EXECUTOR_NAME: &str = "wasmedge";

pub struct WasmEdgeExecutor {
    pub stdin: Option<RawFd>,
    pub stdout: Option<RawFd>,
    pub stderr: Option<RawFd>,
}

impl Executor for WasmEdgeExecutor {
    fn exec(&self, spec: &Spec) -> Result<()> {
        // parse wasi parameters
        let args = get_args(spec);
        if args.is_empty() {
            bail!("args should not be empty")
        }

        let mut cmd = args[0].clone();
        if let Some(stripped) = args[0].strip_prefix(std::path::MAIN_SEPARATOR) {
            cmd = stripped.to_string();
        }
        let envs = env_to_wasi(spec);

        // create configuration with `wasi` option enabled
        let config = ConfigBuilder::new(CommonConfigOptions::default())
            .with_host_registration_config(HostRegistrationConfigOptions::default().wasi(true))
            .build()?;

        // create a vm with the config settings
        let mut vm = Vm::new(Some(config))?;

        // initialize the wasi module with the parsed parameters
        let mut wasi_instance = vm.wasi_module()?;
        wasi_instance.initialize(
            Some(args.iter().map(|s| s as &str).collect()),
            Some(envs.iter().map(|s| s as &str).collect()),
            None,
        );

        let mut vm = vm.register_module_from_file("main", cmd)?;

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

        let ins = vm.named_module("main")?;

        // TODO: How to get exit code?
        // This was relatively straight forward in go, but wasi and wasmtime are totally separate things in rust
        match ins
            .func("_start")
            .expect("Not found '_start' func in the 'main' module instance")
            .call(&mut vm, params!())
        {
            Ok(_) => std::process::exit(0),
            Err(_) => std::process::exit(137),
        };
    }

    fn can_handle(&self, _spec: &Spec) -> Result<bool> {
        Ok(true)
    }

    fn name(&self) -> &'static str {
        EXECUTOR_NAME
    }
}

fn get_args(spec: &Spec) -> &[String] {
    let p = match spec.process() {
        None => return &[],
        Some(p) => p,
    };

    match p.args() {
        None => &[],
        Some(args) => args.as_slice(),
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
