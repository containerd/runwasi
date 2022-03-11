use containerd_shim_wasmtime_v1::sandbox::{shim::Local, ManagerService, WasiInstance};
use containerd_shim_wasmtime_v1::services::sandbox_ttrpc::{create_manager, Manager};
use std::sync::Arc;
use ttrpc::{self, Server};
use wasmtime::{Config, Engine};

fn main() {
    let s: ManagerService<Local<WasiInstance>> =
        ManagerService::new(Engine::new(Config::new().interruptable(true)).unwrap());
    let s = Arc::new(Box::new(s) as Box<dyn Manager + Send + Sync>);
    let service = create_manager(s);

    let mut server = Server::new()
        .bind("unix:///run/io.containerd.wasmtime.v1/manager.sock")
        .unwrap()
        .register_service(service);

    server.start().unwrap();
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    rx.recv().unwrap();
}
