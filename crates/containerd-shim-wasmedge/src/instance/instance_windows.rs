use std::path::PathBuf;
use std::process::ExitCode;

use containerd_shim_wasm::sandbox::{Instance, InstanceConfig};
use containerd_shim_wasm::sandbox::instance::Wait;
use wasmedge_sdk::Vm;
use containerd_shim_wasm::sandbox::error::Error;

pub struct Wasi {
    id: String,

    exit_code: ExitCode,

    stdin: String,
    stdout: String,
    stderr: String,
    bundle: String,

    rootdir: PathBuf,
}

impl Instance for Wasi {
    type Engine = ();

    fn new(id: String, cfg: Option<&InstanceConfig<Self::Engine>>) -> Self {
        todo!()
    }

    fn start(&self) -> std::result::Result<u32, Error> {
        todo!()
    }

    fn kill(&self, signal: u32) -> std::result::Result<(), Error> {
        todo!()
    }

    fn delete(&self) -> std::result::Result<(), Error> {
        todo!()
    }

    fn wait(&self, waiter: &Wait) -> std::result::Result<(), Error> {
        todo!()
    }
}