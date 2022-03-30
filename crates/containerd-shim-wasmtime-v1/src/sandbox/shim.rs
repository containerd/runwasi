use super::instance::{Instance, InstanceConfig, Nop};
use super::{oci, Error, SandboxService};
use chrono::{DateTime, Utc};
use containerd_shim::{
    self as shim, api,
    error::Error as ShimError,
    event::Event,
    mount::mount_rootfs,
    protos::events::task::{TaskCreate, TaskDelete, TaskExit, TaskIO, TaskStart},
    protos::protobuf::well_known_types::Timestamp,
    protos::protobuf::{Message, SingularPtrField},
    protos::shim::shim_ttrpc::Task,
    protos::types::task::Status,
    publisher::RemotePublisher,
    util::IntoOption,
    util::{timestamp as new_timestamp, write_address},
    warn, ExitSignal, TtrpcContext, TtrpcResult,
};
use log::{debug, error};
use nix::mount::{mount, MsFlags};
use nix::sched::{setns, unshare, CloneFlags};
use nix::sys::stat::Mode;
use nix::unistd::mkdir;
use oci_spec::runtime;
use std::collections::HashMap;
use std::env::current_dir;
use std::fs::{self, File};
use std::ops::Not;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use ttrpc::context::Context;
use wasmtime::{Config as EngineConfig, Engine};

struct InstanceData<T: Instance> {
    instance: Option<T>,
    base: Option<Nop>,
    cfg: InstanceConfig,
    pid: RwLock<Option<u32>>,
    status: Arc<RwLock<Option<(u32, DateTime<Utc>)>>>,
}

impl<T> InstanceData<T>
where
    T: Instance,
{
    fn start(&self) -> Result<u32, Error> {
        if self.instance.is_some() {
            return self.instance.as_ref().unwrap().start();
        }
        self.base.as_ref().unwrap().start()
    }

    fn kill(&self, signal: u32) -> Result<(), Error> {
        if self.instance.is_some() {
            return self.instance.as_ref().unwrap().kill(signal);
        }
        self.base.as_ref().unwrap().kill(signal)
    }

    fn delete(&self) -> Result<(), Error> {
        if self.instance.is_some() {
            return self.instance.as_ref().unwrap().delete();
        }
        self.base.as_ref().unwrap().delete()
    }

    fn wait(&self, send: Sender<(u32, DateTime<Utc>)>) -> Result<(), Error> {
        if self.instance.is_some() {
            return self.instance.as_ref().unwrap().wait(send);
        }
        self.base.as_ref().unwrap().wait(send)
    }
}

type EventSender = Sender<(String, Box<dyn Message>)>;

/// Local implements the Task service for a containerd shim.
/// It defers all task operations to the `Instance` implementation.
#[derive(Clone)]
pub struct Local<T: Instance + Send + Sync> {
    engine: Engine,
    instances: Arc<RwLock<HashMap<String, Arc<InstanceData<T>>>>>,
    events: Arc<Mutex<EventSender>>,
    exit: Arc<ExitSignal>,
}

impl<T> Local<T>
where
    T: Instance + Send + Sync,
{
    /// Creates a new local task service.
    pub fn new(
        engine: Engine,
        tx: Sender<(String, Box<dyn Message>)>,
        exit: Arc<ExitSignal>,
    ) -> Self
    where
        T: Instance + Sync + Send,
    {
        Local::<T> {
            // Note: engine.clone() is a shallow clone, is really cheap to do, and is safe to pass around.
            engine: engine.clone(),
            instances: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(Mutex::new(tx)),
            exit: exit,
        }
    }

    fn new_base(&self, id: String) -> InstanceData<T> {
        let cfg = InstanceConfig::new(self.engine.clone());
        InstanceData {
            instance: None,
            base: Some(Nop::new(id, &cfg)),
            cfg: cfg,
            pid: RwLock::new(None),
            status: Arc::new(RwLock::new(None)),
        }
    }

    fn send_event(&self, event: impl Event) {
        let topic = event.topic();
        self.events
            .lock()
            .unwrap()
            .send((topic.clone(), Box::new(event)))
            .unwrap_or_else(|e| warn!("failed to send event for topic {}: {}", topic, e));
    }

    fn get_instance(&self, id: &str) -> Result<Arc<InstanceData<T>>, Error> {
        self.instances
            .read()
            .unwrap()
            .get(id)
            .ok_or_else(|| Error::NotFound(id.to_string()))
            .map(|i| i.clone())
    }

    fn instance_exists(&self, id: &str) -> bool {
        self.instances.read().unwrap().contains_key(id)
    }

    fn is_empty(&self) -> bool {
        self.instances.read().unwrap().is_empty()
    }
}

impl<T> SandboxService for Local<T>
where
    T: Instance + Sync + Send,
{
    type Instance = T;
    fn new(namespace: String, _id: String, engine: Engine, publisher: RemotePublisher) -> Self {
        let (tx, rx) = channel::<(String, Box<dyn Message>)>();
        forward_events(namespace.to_string(), publisher, rx);
        Local::<T>::new(engine, tx.clone(), Arc::new(ExitSignal::default()))
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

        if self.instance_exists(req.get_id()) {
            return Err(Error::AlreadyExists(req.get_id().to_string()).into());
        }

        let mut spec = oci::load(
            Path::new(req.get_bundle())
                .join("config.json")
                .as_path()
                .to_str()
                .unwrap(),
        )
        .map_err(|err| Error::InvalidArgument(format!("could not load runtime spec: {}", err)))?;

        if self.is_empty() {
            // Check if this is a cri container
            // If it is cri, then this is the "pause" container, which we don't need to deal with.
            if spec.annotations().is_some() {
                let annotations = spec.annotations().as_ref().unwrap();
                if annotations.contains_key("io.kubernetes.cri.sandbox-id") {
                    self.instances
                        .write()
                        .unwrap()
                        .insert(req.id.clone(), Arc::new(self.new_base(req.id.clone())));
                    self.send_event(TaskCreate {
                        container_id: req.get_id().into(),
                        bundle: req.get_bundle().into(),
                        rootfs: req.get_rootfs().into(),
                        io: SingularPtrField::some(TaskIO {
                            stdin: req.get_stdin().into(),
                            stdout: req.get_stdout().into(),
                            stderr: req.get_stderr().into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                    return Ok(api::CreateTaskResponse {
                        pid: std::process::id(), // TODO: PID
                        ..Default::default()
                    });
                }
            }
        }

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

        let rootfs_mounts = req.get_rootfs().to_vec();
        if !rootfs_mounts.is_empty() {
            for m in rootfs_mounts {
                let mount_type = m.field_type.as_str().none_if(|&x| x.is_empty());
                let source = m.source.as_str().none_if(|&x| x.is_empty());
                mount_rootfs(mount_type, source, &m.options.to_vec(), rootfs)?;
            }
        }

        let default_mounts = vec![];
        let mounts = spec.mounts().as_ref().unwrap_or(&default_mounts);
        for m in mounts {
            if m.typ().is_some() {
                match m.typ().as_ref().unwrap().as_str() {
                    "tmpfs" | "proc" | "cgroup" | "sysfs" | "devpts" | "mqueue" => continue,
                    _ => (),
                };
            };

            let source = m.source().as_deref().map(|x| x.to_str()).unwrap_or(None);
            let target = m
                .destination()
                .strip_prefix(std::path::MAIN_SEPARATOR.to_string())
                .map_err(|err| {
                    ShimError::InvalidArgument(format!("error stripping path prefix: {}", err))
                })?;

            let rootfs_target = Path::new(rootfs).join(target);

            if source.is_some() {
                let md = fs::metadata(source.unwrap()).map_err(|err| {
                    Error::InvalidArgument(format!("could not get metadata for source: {}", err))
                })?;

                if md.is_dir() {
                    fs::create_dir_all(&rootfs_target).map_err(|err| {
                        ShimError::Other(format!(
                            "error creating directory for mount target {}: {}",
                            target.to_str().unwrap(),
                            err
                        ))
                    })?;
                } else {
                    let parent = rootfs_target.parent();
                    if parent.is_some() {
                        fs::create_dir_all(&parent.unwrap()).map_err(|err| {
                            ShimError::Other(format!(
                                "error creating parent for mount target {}: {}",
                                parent.unwrap().to_str().unwrap(),
                                err
                            ))
                        })?;
                    }
                    File::create(&rootfs_target)
                        .map_err(|err| ShimError::Other(format!("{}", err)))?;
                }
            }

            let mut newopts = vec![];
            let opts = m.options().as_ref();
            if opts.is_some() {
                for o in opts.unwrap() {
                    newopts.push(o.to_string());
                }
            }

            let mut typ = m.typ().as_deref();
            if typ.is_some() {
                if typ.unwrap() == "bind" {
                    typ = None;
                    newopts.push("rbind".to_string());
                }
            }
            mount_rootfs(typ, source, &newopts, &rootfs_target).map_err(|err| {
                ShimError::Other(format!(
                    "error mounting {} to {} as {}: {}",
                    source.unwrap_or_default(),
                    rootfs_target.to_str().unwrap(),
                    m.typ().as_deref().unwrap_or(&"none"),
                    err
                ))
            })?;
        }

        let engine = self.engine.clone();
        let mut builder = InstanceConfig::new(engine);
        builder
            .set_stdin(req.get_stdin().into())
            .set_stdout(req.get_stdout().into())
            .set_stderr(req.get_stderr().into())
            .set_bundle(req.get_bundle().into());
        self.instances.write().unwrap().insert(
            req.get_id().to_string(),
            Arc::new(InstanceData {
                instance: Some(T::new(req.get_id().to_string(), &builder)),
                base: None,
                cfg: builder,
                pid: RwLock::new(None),
                status: Arc::new(RwLock::new(None)),
            }),
        );

        self.send_event(TaskCreate {
            container_id: req.get_id().into(),
            bundle: req.get_bundle().into(),
            rootfs: req.get_rootfs().into(),
            io: SingularPtrField::some(TaskIO {
                stdin: req.get_stdin().into(),
                stdout: req.get_stdout().into(),
                stderr: req.get_stderr().into(),
                ..Default::default()
            }),
            ..Default::default()
        });

        debug!("create done");

        Ok(api::CreateTaskResponse {
            pid: std::process::id(),
            ..Default::default()
        })
    }

    fn start(
        &self,
        _ctx: &::ttrpc::TtrpcContext,
        req: api::StartRequest,
    ) -> TtrpcResult<api::StartResponse> {
        debug!("start: {:?}", req);
        if req.get_exec_id().is_empty().not() {
            return Err(ShimError::Unimplemented("exec is not supported".to_string()).into());
        }

        let i = self.get_instance(req.get_id())?;
        let pid = i.start()?;

        self.send_event(TaskStart {
            container_id: req.get_id().into(),
            pid: pid,
            ..Default::default()
        });

        let mut pid_w = i.pid.write().unwrap();
        *pid_w = Some(pid);
        drop(pid_w);

        let (tx, rx) = channel::<(u32, DateTime<Utc>)>();
        i.wait(tx)?;

        let lock = i.status.clone();
        let sender = self.events.clone();

        let id = req.get_id().to_string();

        thread::Builder::new()
            .name(format!("{}-wait", req.get_id()))
            .spawn(move || {
                let ec = rx.recv().unwrap();

                let mut status = lock.write().unwrap();
                *status = Some(ec);
                drop(status);

                let timestamp = new_timestamp().unwrap();
                let event = TaskExit {
                    container_id: id,
                    exit_status: ec.0,
                    exited_at: SingularPtrField::some(timestamp),
                    ..Default::default()
                };

                let topic = event.topic();
                sender
                    .lock()
                    .unwrap()
                    .send((topic.clone(), Box::new(event)))
                    .unwrap_or_else(|err| {
                        error!("failed to send event for topic {}: {}", topic, err)
                    });
            })
            .map_err(|err| {
                Error::Others(format!("could not spawn thread to wait exit: {}", err))
            })?;

        debug!("started: {:?}", req);

        Ok(api::StartResponse {
            pid: pid,
            ..Default::default()
        })
    }

    fn kill(&self, _ctx: &TtrpcContext, req: api::KillRequest) -> TtrpcResult<api::Empty> {
        if req.get_exec_id().is_empty().not() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()))?;
        }
        debug!("kill: {:?}", req);

        self.get_instance(req.get_id())?.kill(req.get_signal())?;
        Ok(api::Empty::new())
    }

    fn delete(
        &self,
        _ctx: &TtrpcContext,
        req: api::DeleteRequest,
    ) -> TtrpcResult<api::DeleteResponse> {
        debug!("delete: {:?}", req);
        if req.get_exec_id().is_empty().not() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()))?;
        }

        let i = self.get_instance(req.get_id())?;

        i.delete()?;

        let pid = i.pid.read().unwrap().unwrap_or_default();

        let mut event = TaskDelete {
            container_id: req.get_id().into(),
            pid: pid,
            ..Default::default()
        };

        let mut resp = api::DeleteResponse {
            pid: pid,
            ..Default::default()
        };

        let status = i.status.read().unwrap();
        if status.is_some() {
            let ec = status.unwrap();
            event.exit_status = ec.0;
            resp.exit_status = ec.0;

            let mut ts = Timestamp::new();
            ts.set_seconds(ec.1.timestamp());
            ts.set_nanos(ec.1.timestamp_subsec_nanos() as i32);

            let timestamp = new_timestamp()?;
            event.set_exited_at(timestamp.clone());
            resp.set_exited_at(timestamp);
        }
        drop(status);

        self.instances.write().unwrap().remove(req.get_id());

        self.send_event(event);
        Ok(resp)
    }

    fn wait(&self, _ctx: &TtrpcContext, req: api::WaitRequest) -> TtrpcResult<api::WaitResponse> {
        debug!("wait: {:?}", req);
        if req.get_exec_id().is_empty().not() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()))?;
        }

        let i = self.get_instance(req.get_id())?;
        let (tx, rx) = channel::<(u32, DateTime<Utc>)>();
        i.wait(tx)?;

        let code = rx.recv().unwrap();
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
        self.get_instance(req.get_id())?;

        Ok(api::ConnectResponse {
            shim_pid: std::process::id(),
            task_pid: std::process::id(),
            ..Default::default()
        })
    }

    fn state(
        &self,
        _ctx: &TtrpcContext,
        req: api::StateRequest,
    ) -> TtrpcResult<api::StateResponse> {
        debug!("state: {:?}", req);
        if req.get_exec_id().is_empty().not() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()))?;
        }

        let i = self.get_instance(req.get_id())?;
        let mut state = api::StateResponse {
            bundle: i.cfg.get_bundle().unwrap_or_default(),
            stdin: i.cfg.get_stdin().unwrap_or_default(),
            stdout: i.cfg.get_stdout().unwrap_or_default(),
            stderr: i.cfg.get_stderr().unwrap_or_default(),
            ..Default::default()
        };

        let pid_lock = i.pid.read().unwrap();
        let pid = (*pid_lock).clone();
        if pid.is_none() {
            state.status = Status::CREATED;
            return Ok(state);
        }
        drop(pid_lock);

        state.set_pid(pid.unwrap());

        let status = i.status.read().unwrap();

        let code = *status;
        drop(status);

        if code.is_some() {
            state.status = Status::STOPPED;
            let ec = code.unwrap();
            state.exit_status = ec.0;

            let mut timestamp = Timestamp::new();
            timestamp.set_seconds(ec.1.timestamp());
            timestamp.set_nanos(ec.1.timestamp_subsec_nanos() as i32);
            state.set_exited_at(timestamp);
        } else {
            state.status = Status::RUNNING;
        }

        Ok(state)
    }

    fn shutdown(&self, _ctx: &TtrpcContext, _req: api::ShutdownRequest) -> TtrpcResult<api::Empty> {
        debug!("shutdown");

        if self.is_empty().not() {
            return Ok(api::Empty::new());
        }

        self.exit.signal();

        Ok(api::Empty::new())
    }
}

// Cli implements the containerd-shim cli interface using Local<T> as the task service.
pub struct Cli<T: Instance + Sync + Send> {
    pub engine: Engine,
    namespace: String,
    phantom: std::marker::PhantomData<T>,
    exit: Arc<ExitSignal>,
    _id: String,
}

impl<T> shim::Shim for Cli<T>
where
    T: Instance + Sync + Send,
{
    type T = Local<T>;

    fn new(_runtime_id: &str, id: &str, namespace: &str, _config: &mut shim::Config) -> Self {
        let engine = Engine::new(EngineConfig::new().interruptable(true)).unwrap();
        Cli {
            engine,
            phantom: std::marker::PhantomData,
            namespace: namespace.to_string(),
            exit: Arc::new(ExitSignal::default()),
            _id: id.to_string(),
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
                        ShimError::Other(format!(
                            "could not open network namespace {}: {}",
                            ns.path().clone().unwrap().to_str().unwrap(),
                            err
                        ))
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

        let (_child, address) =
            shim::spawn(containerd_shim::StartOpts { ..opts }, &grouping, envs)?;

        write_address(&address)?;

        return Ok(address);
    }

    fn wait(&mut self) {
        self.exit.wait();
    }

    fn create_task_service(&self, publisher: RemotePublisher) -> Self::T {
        let (tx, rx) = channel::<(String, Box<dyn Message>)>();
        forward_events(self.namespace.to_string(), publisher, rx);
        Local::<T>::new(self.engine.clone(), tx.clone(), self.exit.clone())
    }

    fn delete_shim(&mut self) -> shim::Result<api::DeleteResponse> {
        let timestamp = new_timestamp()?;
        Ok(api::DeleteResponse {
            exit_status: 137,
            exited_at: SingularPtrField::some(timestamp),
            ..Default::default()
        })
    }
}

fn forward_events(
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
