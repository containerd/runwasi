use std::path::PathBuf;

use containerd_shim_wasm::sandbox::error::Error;
use containerd_shim_wasm::sandbox::instance::{ExitCode, Wait};
use containerd_shim_wasm::sandbox::{Instance, InstanceConfig, Stdio};

pub struct Wasi {
    id: String,
    exit_code: ExitCode,
    engine: wasmer::Cranelift,
    stdio: Stdio,
    bundle: String,
    rootdir: PathBuf,
}

impl Instance for Wasi {
    type Engine = wasmer::Cranelift;

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
