use containerd_shim_wasm::sandbox::instance::Wait;
use containerd_shim_wasm::sandbox::{Instance, InstanceConfig, Result, Stdio};

pub struct WasmerInstance {}

impl Instance for WasmerInstance {
    type Engine = ();

    fn new(id: String, cfg: Option<&InstanceConfig<Self::Engine>>) -> Self {
        todo!()
    }

    fn start(&self) -> Result<u32> {
        todo!()
    }

    fn kill(&self, signal: u32) -> Result<()> {
        todo!()
    }

    fn delete(&self) -> Result<()> {
        todo!()
    }

    fn wait(&self, waiter: &Wait) -> Result<()> {
        todo!()
    }
}
