use super::instance::{Instance, InstanceConfig, Nop};
use super::{oci, Error, SandboxService};
use containerd_shim::{
    self as shim, api, error::Error as ShimError, mount::mount_rootfs,
    protos::protobuf::well_known_types::Timestamp, protos::protobuf::Message,
    protos::shim::shim_ttrpc::Task, publisher::RemotePublisher, util::write_address,
    util::IntoOption, warn, ExitSignal, TtrpcContext, TtrpcResult,
};
use log::{debug, error, info};
use nix::mount::{mount, MsFlags};
use nix::sched::{setns, unshare, CloneFlags};
use nix::sys::stat::Mode;
use nix::unistd::mkdir;
use oci_spec::runtime;
use std::collections::HashMap;
use std::env::current_dir;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::thread;
use ttrpc::context::Context;
use wasmtime::{Config as EngineConfig, Engine};

#[derive(Clone)]
pub struct Local<T: Instance + Send + Sync> {
    engine: Engine,
    instances: Arc<RwLock<HashMap<String, T>>>,
    base: Arc<RwLock<Option<Nop>>>,
}

impl<T> Local<T>
where
    T: Instance + Send + Sync,
{
    pub fn new(engine: Engine, _tx: Sender<(String, Box<dyn Message>)>) -> Self
    where
        T: Instance + Sync + Send,
    {
        Local {
            // Note: engine.clone() is a shallow clone, is really cheap to do, and is safe to pass around.
            engine: engine.clone(),
            instances: Arc::new(RwLock::new(HashMap::new())),
            base: Arc::new(RwLock::new(None)),
        }
    }

    fn new_base(&self, id: String) -> bool {
        let mut base = self.base.write().unwrap();
        if base.is_none() {
            let nop = Nop::new(id, &InstanceConfig::new(self.engine.clone()));
            *base = Some(nop);
            true
        } else {
            false
        }
    }
}

impl<T> SandboxService for Local<T>
where
    T: Instance + Sync + Send,
{
    type Instance = T;
    fn new(namespace: String, _id: String, engine: Engine, publisher: RemotePublisher) -> Self {
        let (tx, rx) = channel::<(String, Box<dyn Message>)>();
        foward_events(namespace.to_string(), publisher, rx);
        Local::<T>::new(engine, tx.clone())
    }
}

impl<T: Instance + Sync + Send> Task for Local<T> {
    fn create(
        &self,
        _ctx: &TtrpcContext,
        req: api::CreateTaskRequest,
    ) -> TtrpcResult<api::CreateTaskResponse> {
        debug!("create: {:?}", req);
        if !req.get_checkpoint().is_empty() || !req.get_parent_checkpoint().is_empty() {
            return Err(ShimError::Unimplemented("checkpoint is not supported".to_string()).into());
        }

        if req.get_terminal() {
            return Err(Error::InvalidArgument(
                "terminal is not supported".to_string(),
            ))?;
        }

        let mut instances = self.instances.write().unwrap();

        if instances.contains_key(&req.id) {
            return Err(Error::AlreadyExists("aleady exists".to_string()).into());
        }

        let mut spec = oci::load(
            Path::new(req.get_bundle())
                .join("config.json")
                .as_path()
                .to_str()
                .unwrap(),
        )
        .map_err(|err| Error::InvalidArgument(format!("could not load runtime spec: {}", err)))?;

        if instances.len() == 0 {
            // Check if this is a cri container
            // If it is cri, then this is the "pause" container, which we don't need to deal with.
            if !spec.annotations().is_none() {
                let annotations = spec.annotations().as_ref().unwrap();
                if annotations.contains_key("io.kubernetes.cri.sandbox-id") {
                    if !self.new_base(req.id.clone()) {
                        return Err(Error::AlreadyExists("already exists".to_string()))?;
                    };
                    return Ok(api::CreateTaskResponse {
                        pid: 0, // TODO: PID
                        ..Default::default()
                    });
                }
            }
        }

        let rootfs_mounts = req.get_rootfs().to_vec();
        if !rootfs_mounts.is_empty() {
            spec.canonicalize_rootfs(req.get_bundle()).map_err(|err| {
                ShimError::InvalidArgument(format!("could not canonicalize rootfs: {}", err))
            })?;

            let rootfs = spec
                .root()
                .as_ref()
                .ok_or(Error::InvalidArgument(
                    "rootfs is not set in runtime spec".to_string(),
                ))?
                .path();

            match mkdir(rootfs, Mode::from_bits(0o755).unwrap()) {
                Ok(_) => (),
                Err(_) => (),
            };

            for m in rootfs_mounts {
                let mount_type = m.field_type.as_str().none_if(|&x| x.is_empty());
                let source = m.source.as_str().none_if(|&x| x.is_empty());
                mount_rootfs(mount_type, source, &m.options.to_vec(), rootfs)?;
            }
        }

        let engine = self.engine.clone();
        let mut builder = InstanceConfig::new(engine);
        builder
            .set_stdin(req.get_stdin().into())
            .set_stdout(req.get_stdout().into())
            .set_stderr(req.get_stderr().into())
            .set_bundle(req.get_bundle().into());
        instances.insert(
            req.get_id().to_string(),
            T::new(req.get_id().to_string(), &builder),
        );

        debug!("create done");

        Ok(api::CreateTaskResponse {
            pid: 0, // TODO: PID
            ..Default::default()
        })
    }

    fn start(
        &self,
        _ctx: &::ttrpc::TtrpcContext,
        req: api::StartRequest,
    ) -> TtrpcResult<api::StartResponse> {
        debug!("start: {:?}", req);
        if !req.get_exec_id().is_empty() {
            return Err(ShimError::Unimplemented("exec is not supported".to_string()).into());
        }

        let instances = self.instances.read().unwrap();
        let i = instances
            .get(req.get_id())
            .ok_or(Error::NotFound(req.get_id().to_string()))?;

        i.start()?;
        debug!("started: {:?}", req);

        Ok(api::StartResponse {
            pid: 1,
            ..Default::default()
        })
    }

    fn kill(&self, _ctx: &TtrpcContext, req: api::KillRequest) -> TtrpcResult<api::Empty> {
        if !req.get_exec_id().is_empty() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()))?;
        }
        debug!("kill: {:?}", req);

        let instances = self.instances.read().unwrap();
        let i = instances
            .get(req.get_id())
            .ok_or_else(|| Error::NotFound("instance not found".to_string()))?;

        i.kill(req.get_signal())?;

        Ok(api::Empty::new())
    }

    fn delete(
        &self,
        _ctx: &TtrpcContext,
        req: api::DeleteRequest,
    ) -> TtrpcResult<api::DeleteResponse> {
        debug!("delete: {:?}", req);
        if !req.get_exec_id().is_empty() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()))?;
        }

        let mut instances = self.instances.write().unwrap();
        let i = instances
            .get(req.get_id())
            .ok_or(Error::NotFound(req.get_id().to_string()))?;

        i.delete()?;

        instances.remove(req.get_id());

        Ok(api::DeleteResponse {
            pid: 0,
            ..Default::default()
        })
    }

    fn wait(&self, _ctx: &TtrpcContext, req: api::WaitRequest) -> TtrpcResult<api::WaitResponse> {
        debug!("wait: {:?}", req);
        if !req.get_exec_id().is_empty() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()))?;
        }

        let instances = self.instances.write().unwrap();
        let i = instances
            .get(req.get_id())
            .ok_or_else(|| Error::NotFound(req.get_id().to_string()))?;

        let code = i.wait()?;
        debug!("wait done: {:?}", req);

        let mut timestamp = Timestamp::new();
        timestamp.set_seconds(code.1.timestamp());
        timestamp.set_nanos(code.1.timestamp_subsec_nanos() as i32);

        let mut wr = api::WaitResponse {
            exit_status: code.0,
            ..Default::default()
        };
        wr.set_exited_at(timestamp);
        Ok(wr)
    }

    fn connect(
        &self,
        _ctx: &TtrpcContext,
        req: api::ConnectRequest,
    ) -> TtrpcResult<api::ConnectResponse> {
        debug!("connect: {:?}", req);
        let instances = self.instances.read().unwrap();
        instances
            .get(req.get_id())
            .ok_or_else(|| Error::NotFound(req.get_id().to_string()))?;

        Ok(api::ConnectResponse {
            shim_pid: std::process::id(),
            task_pid: std::process::id(),
            ..Default::default()
        })
    }
}

pub struct Cli<T: Instance + Sync + Send> {
    pub engine: Engine,
    namespace: String,
    phantom: std::marker::PhantomData<T>,
    exit: Arc<ExitSignal>,
}

impl<T> shim::Shim for Cli<T>
where
    T: Instance + Sync + Send,
{
    type T = Local<T>;

    fn new(_runtime_id: &str, _id: &str, namespace: &str, _config: &mut shim::Config) -> Self {
        let engine = Engine::new(EngineConfig::new().interruptable(true)).unwrap();
        Cli {
            engine,
            phantom: std::marker::PhantomData,
            namespace: namespace.to_string(),
            exit: Arc::new(ExitSignal::default()),
        }
    }

    fn start_shim(&mut self, opts: containerd_shim::StartOpts) -> shim::Result<String> {
        let dir = current_dir().map_err(|err| ShimError::Other(err.to_string()))?;
        let spec = oci::load(dir.join("config.json").to_str().unwrap()).map_err(|err| {
            shim::Error::InvalidArgument(format!("error loading runtime spec: {}", err))
        })?;

        let default = HashMap::new() as HashMap<String, String>;
        let annotations = spec.annotations().as_ref().unwrap_or_else(|| &default);

        let id = opts.id.clone();

        let grouping = annotations
            .get("io.kubernetes.cri.sandbox-id")
            .unwrap_or(&id)
            .to_string();

        let envs = vec![] as Vec<(&str, &str)>;

        let namespaces = spec
            .linux()
            .as_ref()
            .unwrap()
            .namespaces()
            .as_ref()
            .unwrap();
        for ns in namespaces {
            if ns.typ() == runtime::LinuxNamespaceType::Network {
                if ns.path().is_some() {
                    let p = ns.path().clone().unwrap();
                    let f = File::open(p).map_err(|err| {
                        ShimError::Other(format!("could not open network namespace: {0}", err))
                    })?;
                    setns(f.as_raw_fd(), CloneFlags::CLONE_NEWNET).map_err(|err| {
                        ShimError::Other(format!("could not set network namespace: {0}", err))
                    })?;
                    break;
                }
                unshare(CloneFlags::CLONE_NEWNET).map_err(|err| {
                    ShimError::Other(format!("could not unshare network namespace: {0}", err))
                })?;
            }
        }

        // Keep all mounts chanmges (such as for the rootfs) private to the shim
        // This way mounts will automatically be cleaned up when the shim exits.
        unshare(CloneFlags::CLONE_NEWNS).map_err(|err| {
            shim::Error::Other(format!("failed to unshare mount namespace: {}", err))
        })?;

        mount::<str, Path, str, str>(
            None,
            "/".as_ref(),
            None,
            MsFlags::MS_REC | MsFlags::MS_SLAVE,
            None,
        )
        .map_err(|err| shim::Error::Other(format!("failed to remount rootfs as slave: {}", err)))?;

        let (_child, address) = shim::spawn(opts, &grouping, envs)?;

        write_address(&address)?;

        return Ok(address);
    }

    fn wait(&mut self) {
        self.exit.wait();
    }

    fn create_task_service(&self, publisher: RemotePublisher) -> Self::T {
        let (tx, rx) = channel::<(String, Box<dyn Message>)>();
        foward_events(self.namespace.to_string(), publisher, rx);
        Local::<T>::new(self.engine.clone(), tx.clone())
    }

    fn delete_shim(&mut self) -> shim::Result<api::DeleteResponse> {
        todo!()
    }
}

fn foward_events(
    namespace: String,
    publisher: RemotePublisher,
    events: Receiver<(String, Box<dyn Message>)>,
) {
    thread::Builder::new()
        .name("event-publisher".to_string())
        .spawn(move || {
            for (topic, event) in events.iter() {
                publisher
                    .publish(Context::default(), &topic, &namespace, event)
                    .unwrap_or_else(|e| warn!("failed to publish event, topic: {}: {}", &topic, e));
            }
        })
        .unwrap();
}
