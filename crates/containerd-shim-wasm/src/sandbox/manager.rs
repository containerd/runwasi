use std::collections::HashMap;
use std::env::current_dir;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;
use std::thread;

use anyhow::Context;
use containerd_shim::{
    self as shim, api,
    error::Error as ShimError,
    protos::shim::shim_ttrpc::{create_task, Task},
    protos::ttrpc::{Client, Server},
    protos::TaskClient,
    publisher::RemotePublisher,
    TtrpcContext, TtrpcResult,
};
use nix::sched::{setns, unshare, CloneFlags};
use oci_spec::runtime;
use ttrpc::context;

use super::error::Error;
use super::instance::Instance;
use super::oci;
use super::sandbox;
use crate::services::sandbox_ttrpc::{Manager, ManagerClient};

pub trait Sandbox<E>: Task + Send + Sync
where
    E: Send + Sync + Clone,
{
    type Instance: Instance<E = E>;

    fn new(namespace: String, id: String, engine: E, publisher: RemotePublisher) -> Self;
}

pub struct Service<E, T>
where
    E: Send + Sync + Clone,
    T: Sandbox<E>,
{
    sandboxes: RwLock<HashMap<String, String>>,
    engine: E,
    phantom: std::marker::PhantomData<T>,
}

impl<E, T> Service<E, T>
where
    E: Send + Sync + Clone,
    T: Sandbox<E>,
{
    pub fn new(engine: E) -> Self {
        Service::<E, T> {
            sandboxes: RwLock::new(HashMap::new()),
            engine,
            phantom: std::marker::PhantomData,
        }
    }
}

impl<E, T> Manager for Service<E, T>
where
    T: Sandbox<E> + 'static,
    E: Send + Sync + Clone,
{
    fn create(
        &self,
        _ctx: &TtrpcContext,
        req: sandbox::CreateRequest,
    ) -> TtrpcResult<sandbox::CreateResponse> {
        let mut sandboxes = self.sandboxes.write().unwrap();

        if sandboxes.contains_key(&req.id) {
            return Err(Error::AlreadyExists(req.get_id().to_string()).into());
        }

        let sock = format!("unix://{}/shim.sock", &req.working_directory);

        let publisher = RemotePublisher::new(req.ttrpc_address)?;

        let sb = T::new(
            req.namespace.clone(),
            req.id.clone(),
            self.engine.clone(),
            publisher,
        );
        let task_service = create_task(Arc::new(Box::new(sb)));
        let mut server = Server::new().bind(&sock)?.register_service(task_service);

        sandboxes.insert(req.id.clone(), sock.clone());

        let cfg = oci::spec_from_file(
            Path::new(&req.working_directory)
                .join("config.json")
                .to_str()
                .unwrap(),
        )
        .map_err(|err| Error::InvalidArgument(format!("could not load runtime spec: {}", err)))?;

        let (tx, rx) = std::sync::mpsc::channel::<Result<(), Error>>();

        let id = &req.id;

        match thread::Builder::new()
            .name(format!("{}-sandbox-create", id))
            .spawn(move || {
                let r = start_sandbox(cfg, &mut server);
                tx.send(r).context("could not send sandbox result").unwrap();
            }) {
            Ok(_) => {}
            Err(e) => {
                return Err(Error::Others(format!("failed to spawn sandbox thread: {}", e)).into());
            }
        }

        rx.recv()
            .context("could not receive sandbox result")
            .map_err(|err| Error::Others(format!("{}", err)))??;
        Ok(sandbox::CreateResponse {
            socket_path: sock,
            ..Default::default()
        })
    }

    fn delete(
        &self,
        _ctx: &TtrpcContext,
        req: sandbox::DeleteRequest,
    ) -> TtrpcResult<sandbox::DeleteResponse> {
        let mut sandboxes = self.sandboxes.write().unwrap();
        if !sandboxes.contains_key(&req.id) {
            return Err(Error::NotFound(req.get_id().to_string()).into());
        }
        let sock = sandboxes.remove(&req.id).unwrap();
        let c = Client::connect(&sock)?;
        let tc = TaskClient::new(c);

        tc.shutdown(
            context::Context::default(),
            &api::ShutdownRequest {
                id: req.id,
                now: true,
                ..Default::default()
            },
        )?;

        Ok(sandbox::DeleteResponse::default())
    }
}

// Note that this changes the current thread's state.
// You probably want to run this in a new thread.
fn start_sandbox(cfg: runtime::Spec, server: &mut Server) -> Result<(), Error> {
    let namespaces = cfg.linux().as_ref().unwrap().namespaces().as_ref().unwrap();
    for ns in namespaces {
        if ns.typ() == runtime::LinuxNamespaceType::Network {
            if ns.path().is_some() {
                let p = ns.path().clone().unwrap();
                let f = File::open(p).context("could not open network namespace")?;
                setns(f.as_raw_fd(), CloneFlags::CLONE_NEWNET)
                    .context("error setting network namespace")?;
                break;
            }

            unshare(CloneFlags::CLONE_NEWNET).context("error unsharing network namespace")?;
        }
    }

    server.start_listen().context("could not start listener")?;
    Ok(())
}

/// Shim implements the containerd-shim CLI for connecting to a Manager service.
pub struct Shim {
    id: String,
    namespace: String,
}

impl Task for Shim {}

impl shim::Shim for Shim {
    type T = Self;

    fn new(_runtime_id: &str, id: &str, namespace: &str, _config: &mut shim::Config) -> Self {
        Shim {
            id: id.to_string(),
            namespace: namespace.to_string(),
        }
    }

    fn start_shim(&mut self, opts: containerd_shim::StartOpts) -> shim::Result<String> {
        let dir = current_dir().map_err(|err| ShimError::Other(err.to_string()))?;
        let spec = oci::load(dir.join("config.json").to_str().unwrap()).map_err(|err| {
            shim::Error::InvalidArgument(format!("error loading runtime spec: {}", err))
        })?;

        let default = HashMap::new() as HashMap<String, String>;
        let annotations = spec.annotations().as_ref().unwrap_or(&default);

        let sandbox = annotations
            .get("io.kubernetes.cri.sandbox-id")
            .unwrap_or(&opts.id)
            .to_string();

        let client = Client::connect("unix:///run/io.containerd.wasmtime.v1/manager.sock")?;
        let mc = ManagerClient::new(client);

        let addr = match mc.create(
            context::Context::default(),
            &sandbox::CreateRequest {
                id: sandbox.clone(),
                working_directory: dir.as_path().to_str().unwrap().to_string(),
                ttrpc_address: opts.ttrpc_address.clone(),
                ..Default::default()
            },
        ) {
            Ok(res) => res.get_socket_path().to_string(),
            Err(_) => {
                let res = mc.connect(
                    context::Context::default(),
                    &sandbox::ConnectRequest {
                        id: sandbox,
                        ttrpc_address: opts.ttrpc_address,
                        ..Default::default()
                    },
                )?;
                res.get_socket_path().to_string()
            }
        };

        shim::util::write_address(&addr)?;

        Ok(addr)
    }

    fn wait(&mut self) {
        todo!()
    }

    fn create_task_service(&self, _publisher: RemotePublisher) -> Self::T {
        todo!() // but not really, haha
    }

    fn delete_shim(&mut self) -> shim::Result<api::DeleteResponse> {
        let dir = current_dir().map_err(|err| ShimError::Other(err.to_string()))?;
        let spec = oci::load(dir.join("config.json").to_str().unwrap()).map_err(|err| {
            shim::Error::InvalidArgument(format!("error loading runtime spec: {}", err))
        })?;

        let default = HashMap::new() as HashMap<String, String>;
        let annotations = spec.annotations().as_ref().unwrap_or(&default);

        let sandbox = annotations
            .get("io.kubernetes.cri.sandbox-id")
            .unwrap_or(&self.id)
            .to_string();
        if sandbox != self.id {
            return Ok(api::DeleteResponse::default());
        }

        let client = Client::connect("unix:///run/io.containerd.wasmtime.v1/manager.sock")?;
        let mc = ManagerClient::new(client);
        mc.delete(
            context::Context::default(),
            &sandbox::DeleteRequest {
                id: sandbox,
                namespace: self.namespace.clone(),
                ..Default::default()
            },
        )?;

        // TODO: write pid, exit code, etc to disk so we can use it here.
        Ok(api::DeleteResponse::default())
    }
}
