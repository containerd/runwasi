use std::sync::Arc;

use containerd_shim_wasm::sandbox::{Local, ManagerService};
use containerd_shim_wasm::services::sandbox_ttrpc::{create_manager, Manager};
use containerd_shim_wasmtime::WasmtimeInstance;
use log::info;
use ttrpc::{self, Server};

fn main() {
    info!("starting up!");
    let s: ManagerService<Local<WasmtimeInstance>> = ManagerService::default();
    let s = Arc::new(Box::new(s) as Box<dyn Manager + Send + Sync>);
    let service = create_manager(s);

    let mut server = Server::new()
        .bind("unix:///run/io.containerd.wasmwasi.v1/manager.sock")
        .unwrap()
        .register_service(service);

    server.start().unwrap();
    info!("server started!");
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    rx.recv().unwrap();
}
