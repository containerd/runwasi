use std::path::PathBuf;

use containerd_shim_wasm::sandbox::error::Error;
use containerd_shim_wasm::sandbox::instance::{ExitCode, Wait};
use containerd_shim_wasm::sandbox::{Instance, InstanceConfig};

pub struct Wasi {
    exit_code: ExitCode,
    engine: wasmtime::Engine,
    stdin: String,
    stdout: String,
    stderr: String,
    bundle: String,
    rootdir: PathBuf,
    id: String,
}

impl Instance for Wasi {
    type Engine = wasmtime::Engine;

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

    fn wait(
        &self,
        waiter: &containerd_shim_wasm::sandbox::instance::Wait,
    ) -> std::result::Result<(), Error> {
        todo!()
    }
}
