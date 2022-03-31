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

type Result<T> = std::result::Result<T, Error>;

impl<T> InstanceData<T>
where
    T: Instance,
{
    fn start(&self) -> Result<u32> {
        if self.instance.is_some() {
            return self.instance.as_ref().unwrap().start();
        }
        self.base.as_ref().unwrap().start()
    }

    fn kill(&self, signal: u32) -> Result<()> {
        if self.instance.is_some() {
            return self.instance.as_ref().unwrap().kill(signal);
        }
        self.base.as_ref().unwrap().kill(signal)
    }

    fn delete(&self) -> Result<()> {
        if self.instance.is_some() {
            return self.instance.as_ref().unwrap().delete();
        }
        self.base.as_ref().unwrap().delete()
    }

    fn wait(&self, send: Sender<(u32, DateTime<Utc>)>) -> Result<()> {
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

#[cfg(test)]
mod localtests {
    use super::*;
    use anyhow::Context;
    use serde_json as json;
    use std::fs::create_dir;
    use std::time::Duration;
    use tempfile::tempdir;

    struct LocalWithDescrutor<T: Instance + Send + Sync> {
        local: Arc<Local<T>>,
    }

    impl<T> LocalWithDescrutor<T>
    where
        T: Instance + Send + Sync,
    {
        fn new(local: Arc<Local<T>>) -> Self {
            Self { local }
        }
    }

    impl<T> Drop for LocalWithDescrutor<T>
    where
        T: Instance + Send + Sync,
    {
        fn drop(&mut self) {
            self.local
                .instances
                .write()
                .unwrap()
                .iter()
                .for_each(|(_, v)| {
                    v.kill(9).unwrap();
                    v.delete().unwrap();
                });
        }
    }

    fn with_cri_sandbox(spec: Option<runtime::Spec>, id: String) -> runtime::Spec {
        let mut s = spec.unwrap_or(runtime::Spec::default());
        let mut annotations = HashMap::new();
        s.annotations().as_ref().map(|a| {
            a.iter().map(|(k, v)| {
                annotations.insert(k.to_string(), v.to_string());
            })
        });
        annotations.insert("io.kubernetes.cri.sandbox-id".to_string(), id);

        s.set_annotations(Some(annotations));
        return s;
    }

    fn create_bundle(dir: &std::path::Path, spec: Option<runtime::Spec>) -> Result<()> {
        create_dir(dir.join("rootfs"))?;

        let s = spec.unwrap_or(runtime::Spec::default());

        json::to_writer(File::create(dir.join("config.json"))?, &s)
            .context("could not write config.json")?;
        Ok(())
    }

    #[test]
    fn test_cri_task() -> Result<()> {
        // Currently the relationship between the "base" container and the "instances" are pretty weak.
        // When a cri sandbox is specified we just assume it's the sandbox container and treat it as such by not actually running the code (which is going to be wasm).
        let (etx, _erx) = channel();
        let exit_signal = Arc::new(ExitSignal::default());
        let local = Arc::new(Local::<Nop>::new(
            Engine::new(&EngineConfig::default())?,
            etx,
            exit_signal.clone(),
        ));

        let mut _wrapped = LocalWithDescrutor::new(local.clone());

        let temp = tempdir().unwrap();
        let dir = temp.path();
        let sandbox_id = "test-cri-task".to_string();
        create_bundle(&dir, Some(with_cri_sandbox(None, sandbox_id.clone())))?;

        local.create_task(api::CreateTaskRequest {
            id: "testbase".to_string(),
            bundle: dir.to_str().unwrap().to_string(),
            ..Default::default()
        })?;

        let state = local.task_state(api::StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status, Status::CREATED);

        // A little janky since this is internal data, but check that this is seen as a sandbox container
        let i = local.get_instance("testbase")?;
        assert!(i.base.is_some());
        assert!(i.instance.is_none());

        local.start_task(api::StartRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })?;

        let state = local.task_state(api::StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status, Status::RUNNING);

        let ll = local.clone();
        let (base_tx, base_rx) = channel();
        thread::spawn(move || {
            let resp = ll.wait_task(api::WaitRequest {
                id: "testbase".to_string(),
                ..Default::default()
            });
            base_tx.send(resp).unwrap();
        });
        base_rx.try_recv().unwrap_err();

        let temp2 = tempdir().unwrap();
        let dir2 = temp2.path();
        create_bundle(&dir2, Some(with_cri_sandbox(None, sandbox_id.clone())))?;

        local.create_task(api::CreateTaskRequest {
            id: "testinstance".to_string(),
            bundle: dir2.to_str().unwrap().to_string(),
            ..Default::default()
        })?;

        let state = local.task_state(api::StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status, Status::CREATED);

        // again, this is janky since it is internal data, but check that this is seen as a "real" container.
        // this is the inverse of the above test case.
        let i = local.get_instance("testinstance")?;
        assert!(i.base.is_none());
        assert!(i.instance.is_some());

        local.start_task(api::StartRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })?;

        let state = local.task_state(api::StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status, Status::RUNNING);

        let ll = local.clone();
        let (instance_tx, instance_rx) = channel();
        std::thread::spawn(move || {
            let resp = ll.wait_task(api::WaitRequest {
                id: "testinstance".to_string(),
                ..Default::default()
            });
            instance_tx.send(resp).unwrap();
        });
        instance_rx.try_recv().unwrap_err();

        local.kill_task(api::KillRequest {
            id: "testinstance".to_string(),
            signal: 9,
            ..Default::default()
        })?;

        instance_rx.recv_timeout(Duration::from_secs(5)).unwrap()?;

        let state = local.task_state(api::StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status, Status::STOPPED);
        local.delete_task(api::DeleteRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })?;

        match local
            .task_state(api::StateRequest {
                id: "testinstance".to_string(),
                ..Default::default()
            })
            .unwrap_err()
        {
            Error::NotFound(_) => {}
            e => return Err(e),
        }

        base_rx.try_recv().unwrap_err();
        let state = local.task_state(api::StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status, Status::RUNNING);

        local.kill_task(api::KillRequest {
            id: "testbase".to_string(),
            signal: 9,
            ..Default::default()
        })?;

        base_rx.recv_timeout(Duration::from_secs(5)).unwrap()?;
        let state = local.task_state(api::StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status, Status::STOPPED);

        local.delete_task(api::DeleteRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })?;
        match local
            .task_state(api::StateRequest {
                id: "testbase".to_string(),
                ..Default::default()
            })
            .unwrap_err()
        {
            Error::NotFound(_) => {}
            e => return Err(e),
        }

        Ok(())
    }

    #[test]
    fn test_task_lifecycle() -> Result<()> {
        let (etx, _erx) = channel(); // TODO: check events
        let exit_signal = Arc::new(ExitSignal::default());
        let local = Arc::new(Local::<Nop>::new(
            Engine::new(&EngineConfig::default())?,
            etx,
            exit_signal.clone(),
        ));

        let mut _wrapped = LocalWithDescrutor::new(local.clone());

        let temp = tempdir().unwrap();
        let dir = temp.path();
        create_bundle(dir, None)?;

        match local
            .task_state(api::StateRequest {
                id: "test".to_string(),
                ..Default::default()
            })
            .unwrap_err()
        {
            Error::NotFound(_) => {}
            e => return Err(e),
        }

        local.create_task(api::CreateTaskRequest {
            id: "test".to_string(),
            bundle: dir.to_str().unwrap().to_string(),
            ..Default::default()
        })?;

        match local
            .create_task(api::CreateTaskRequest {
                id: "test".to_string(),
                bundle: dir.to_str().unwrap().to_string(),
                ..Default::default()
            })
            .unwrap_err()
        {
            Error::AlreadyExists(_) => {}
            e => return Err(e),
        }

        let state = local.task_state(api::StateRequest {
            id: "test".to_string(),
            ..Default::default()
        })?;

        assert_eq!(state.get_status(), Status::CREATED);

        local.start_task(api::StartRequest {
            id: "test".to_string(),
            ..Default::default()
        })?;

        let state = local.task_state(api::StateRequest {
            id: "test".to_string(),
            ..Default::default()
        })?;

        assert_eq!(state.get_status(), Status::RUNNING);

        let (tx, rx) = channel();
        let ll = local.clone();
        thread::spawn(move || {
            let resp = ll.wait_task(api::WaitRequest {
                id: "test".to_string(),
                ..Default::default()
            });
            tx.send(resp).unwrap();
        });

        rx.try_recv().unwrap_err();

        local.kill_task(api::KillRequest {
            id: "test".to_string(),
            signal: 9,
            ..Default::default()
        })?;

        rx.recv_timeout(Duration::from_secs(5)).unwrap()?;

        let state = local.task_state(api::StateRequest {
            id: "test".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.get_status(), Status::STOPPED);

        local.delete_task(api::DeleteRequest {
            id: "test".to_string(),
            ..Default::default()
        })?;

        match local
            .task_state(api::StateRequest {
                id: "test".to_string(),
                ..Default::default()
            })
            .unwrap_err()
        {
            Error::NotFound(_) => {}
            e => return Err(e),
        }

        Ok(())
    }
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

    fn get_instance(&self, id: &str) -> Result<Arc<InstanceData<T>>> {
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

    fn create_task(&self, req: api::CreateTaskRequest) -> Result<api::CreateTaskResponse> {
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
            //
            // TODO: maybe we can just go ahead and execute the actual container with runc?
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

    fn start_task(&self, req: api::StartRequest) -> Result<api::StartResponse> {
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

    fn kill_task(&self, req: api::KillRequest) -> Result<()> {
        if req.get_exec_id().is_empty().not() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()))?;
        }
        self.get_instance(req.get_id())?.kill(req.get_signal())?;
        Ok(())
    }

    fn delete_task(&self, req: api::DeleteRequest) -> Result<api::DeleteResponse> {
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

    fn wait_task(&self, req: api::WaitRequest) -> Result<api::WaitResponse> {
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

    fn task_state(&self, req: api::StateRequest) -> Result<api::StateResponse> {
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
        let resp = self.create_task(req)?;
        Ok(resp)
    }

    fn start(
        &self,
        _ctx: &::ttrpc::TtrpcContext,
        req: api::StartRequest,
    ) -> TtrpcResult<api::StartResponse> {
        debug!("start: {:?}", req);
        let resp = self.start_task(req)?;
        Ok(resp)
    }

    fn kill(&self, _ctx: &TtrpcContext, req: api::KillRequest) -> TtrpcResult<api::Empty> {
        debug!("kill: {:?}", req);
        self.kill_task(req)?;
        Ok(api::Empty::new())
    }

    fn delete(
        &self,
        _ctx: &TtrpcContext,
        req: api::DeleteRequest,
    ) -> TtrpcResult<api::DeleteResponse> {
        debug!("delete: {:?}", req);
        let resp = self.delete_task(req)?;
        Ok(resp)
    }

    fn wait(&self, _ctx: &TtrpcContext, req: api::WaitRequest) -> TtrpcResult<api::WaitResponse> {
        debug!("wait: {:?}", req);
        let resp = self.wait_task(req)?;
        Ok(resp)
    }

    fn connect(
        &self,
        _ctx: &TtrpcContext,
        req: api::ConnectRequest,
    ) -> TtrpcResult<api::ConnectResponse> {
        debug!("connect: {:?}", req);

        let i = self.get_instance(req.get_id())?;
        let pid = *i.pid.read().unwrap().as_ref().unwrap_or(&0);

        Ok(api::ConnectResponse {
            shim_pid: std::process::id(),
            task_pid: pid,
            ..Default::default()
        })
    }

    fn state(
        &self,
        _ctx: &TtrpcContext,
        req: api::StateRequest,
    ) -> TtrpcResult<api::StateResponse> {
        debug!("state: {:?}", req);
        let resp = self.task_state(req)?;
        Ok(resp)
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
