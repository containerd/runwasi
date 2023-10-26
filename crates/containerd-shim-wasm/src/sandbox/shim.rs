//! The shim is the entrypoint for the containerd shim API. It is responsible
//! for commmuincating with the containerd daemon and managing the lifecycle of
//! the container/sandbox.

use std::collections::HashMap;
use std::env::current_dir;
use std::fs::{self, DirBuilder, File};
use std::ops::Not;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use anyhow::Context as AnyhowContext;
use chrono::{DateTime, Utc};
use containerd_shim::error::Error as ShimError;
use containerd_shim::event::Event;
#[cfg(unix)]
use containerd_shim::mount::mount_rootfs;
use containerd_shim::protos::events::task::{TaskCreate, TaskDelete, TaskExit, TaskIO, TaskStart};
use containerd_shim::protos::protobuf::well_known_types::timestamp::Timestamp;
use containerd_shim::protos::protobuf::{MessageDyn, MessageField};
use containerd_shim::protos::shim::shim_ttrpc::Task;
use containerd_shim::protos::types::task::Status;
use containerd_shim::publisher::RemotePublisher;
use containerd_shim::util::{timestamp as new_timestamp, write_address, IntoOption};
use containerd_shim::{self as shim, api, warn, ExitSignal, TtrpcContext, TtrpcResult};
use log::{debug, error};
#[cfg(unix)]
use nix::mount::{mount, MsFlags};
use oci_spec::runtime::Spec;
use shim::api::{StatsRequest, StatsResponse};
use shim::Flags;
use ttrpc::context::Context;

use super::instance::{Instance, InstanceConfig, Nop};
use super::{oci, Error, SandboxService};
use crate::sys::metrics::get_metrics;
use crate::sys::networking::setup_namespaces;

enum InstanceOption<I: Instance> {
    Instance(I),
    Nop(Nop),
}

impl<I: Instance> Instance for InstanceOption<I> {
    type Engine = ();

    fn new(_id: String, _cfg: Option<&InstanceConfig<Self::Engine>>) -> Result<Self> {
        // this is never called
        unimplemented!();
    }

    fn start(&self) -> Result<u32> {
        match self {
            Self::Instance(i) => i.start(),
            Self::Nop(i) => i.start(),
        }
    }

    fn kill(&self, signal: u32) -> Result<()> {
        match self {
            Self::Instance(i) => i.kill(signal),
            Self::Nop(i) => i.kill(signal),
        }
    }

    fn delete(&self) -> Result<()> {
        match self {
            Self::Instance(i) => i.delete(),
            Self::Nop(i) => i.delete(),
        }
    }

    fn wait_timeout(&self, t: impl Into<Option<Duration>>) -> Option<(u32, DateTime<Utc>)> {
        match self {
            Self::Instance(i) => i.wait_timeout(t),
            Self::Nop(i) => i.wait_timeout(t),
        }
    }
}

struct InstanceData<T: Instance> {
    instance: InstanceOption<T>,
    cfg: InstanceConfig<T::Engine>,
    pid: RwLock<Option<u32>>,
    state: Arc<RwLock<TaskState>>,
}

type Result<T, E = Error> = std::result::Result<T, E>;

impl<T: Instance> InstanceData<T> {
    fn start(&self) -> Result<u32> {
        let mut s = self.state.write().unwrap();
        s.start()?;

        let res = self.instance.start();

        // These state transitions are always `Ok(())` because
        // we hold the lock since `s.start()`
        let _ = match res {
            Ok(_) => s.started(),
            Err(_) => s.stop(),
        };

        res
    }

    fn kill(&self, signal: u32) -> Result<()> {
        let mut s = self.state.write().unwrap();
        s.kill()?;

        self.instance.kill(signal)
    }

    fn delete(&self) -> Result<()> {
        let mut s = self.state.write().unwrap();
        s.delete()?;

        let res = self.instance.delete();

        if res.is_err() {
            // Always `Ok(())` because we hold the lock since `s.delete()`
            let _ = s.stop();
        }

        res
    }

    fn wait(&self) -> (u32, DateTime<Utc>) {
        let res = self.instance.wait();
        let mut s = self.state.write().unwrap();
        *s = TaskState::Exited;
        res
    }

    fn wait_timeout(&self, t: impl Into<Option<Duration>>) -> Option<(u32, DateTime<Utc>)> {
        let res = self.instance.wait_timeout(t);
        if res.is_some() {
            let mut s = self.state.write().unwrap();
            *s = TaskState::Exited;
        }
        res
    }
}

type EventSender = Sender<(String, Box<dyn MessageDyn>)>;

#[derive(Debug, Clone, Copy)]
enum TaskState {
    Created,
    Starting,
    Started,
    Exited,
    Deleting,
}

impl TaskState {
    pub fn start(&mut self) -> Result<()> {
        *self = match self {
            Self::Created => Ok(Self::Starting),
            _ => state_transition_error(*self, Self::Starting),
        }?;
        Ok(())
    }

    pub fn kill(&mut self) -> Result<()> {
        *self = match self {
            Self::Started => Ok(Self::Started),
            _ => state_transition_error(*self, "Killing"),
        }?;
        Ok(())
    }

    pub fn delete(&mut self) -> Result<()> {
        *self = match self {
            Self::Created | Self::Exited => Ok(Self::Deleting),
            _ => state_transition_error(*self, Self::Deleting),
        }?;
        Ok(())
    }

    pub fn started(&mut self) -> Result<()> {
        *self = match self {
            Self::Starting => Ok(Self::Started),
            _ => state_transition_error(*self, Self::Started),
        }?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        *self = match self {
            Self::Started | Self::Starting => Ok(Self::Exited),
            // This is for potential failure cases where we want delete to be able to be retried.
            Self::Deleting => Ok(Self::Exited),
            _ => state_transition_error(*self, Self::Exited),
        }?;
        Ok(())
    }
}

fn state_transition_error<T>(from: impl std::fmt::Debug, to: impl std::fmt::Debug) -> Result<T> {
    Err(Error::FailedPrecondition(format!(
        "invalid state transition: {from:?} => {to:?}"
    )))
}

type LocalInstances<T> = Arc<RwLock<HashMap<String, Arc<InstanceData<T>>>>>;

/// Local implements the Task service for a containerd shim.
/// It defers all task operations to the `Instance` implementation.
#[derive(Clone)]
pub struct Local<T: Instance + Send + Sync> {
    engine: T::Engine,
    instances: LocalInstances<T>,
    events: Arc<Mutex<EventSender>>,
    exit: Arc<ExitSignal>,
    namespace: String,
    containerd_address: String,
}

#[cfg(test)]
mod localtests {
    use std::fs::create_dir;
    use std::time::Duration;

    use anyhow::Context;
    use oci_spec::runtime;
    use serde_json as json;
    use tempfile::tempdir;

    use super::*;

    struct LocalWithDescrutor<T: Instance + Send + Sync> {
        local: Arc<Local<T>>,
    }

    impl<T: Instance + Send + Sync> LocalWithDescrutor<T> {
        fn new(local: Arc<Local<T>>) -> Self {
            Self { local }
        }
    }

    impl<T: Instance + Send + Sync> Drop for LocalWithDescrutor<T> {
        fn drop(&mut self) {
            self.local
                .instances
                .write()
                .unwrap()
                .iter()
                .for_each(|(_, v)| {
                    let _ = v.kill(9);
                    v.delete().unwrap();
                });
        }
    }

    fn with_cri_sandbox(spec: Option<runtime::Spec>, id: String) -> runtime::Spec {
        let mut s = spec.unwrap_or_default();
        let mut annotations = HashMap::new();
        s.annotations().as_ref().map(|a| {
            a.iter().map(|(k, v)| {
                annotations.insert(k.to_string(), v.to_string());
            })
        });
        annotations.insert("io.kubernetes.cri.sandbox-id".to_string(), id);

        s.set_annotations(Some(annotations));
        s
    }

    fn create_bundle(dir: &std::path::Path, spec: Option<runtime::Spec>) -> Result<()> {
        create_dir(dir.join("rootfs"))?;

        let s = spec.unwrap_or_default();

        json::to_writer(File::create(dir.join("config.json"))?, &s)
            .context("could not write config.json")?;
        Ok(())
    }

    #[test]
    fn test_delete_after_create() {
        let dir = tempdir().unwrap();
        let id = "test-delete-after-create";
        create_bundle(dir.path(), None).unwrap();

        let (tx, _rx) = channel();
        let local = Arc::new(Local::<Nop>::new(
            (),
            tx,
            Arc::new(ExitSignal::default()),
            "test_namespace".into(),
            "/test/address".into(),
        ));
        let mut _wrapped = LocalWithDescrutor::new(local.clone());

        local
            .task_create(api::CreateTaskRequest {
                id: id.to_string(),
                bundle: dir.path().to_str().unwrap().to_string(),
                ..Default::default()
            })
            .unwrap();

        local
            .task_delete(api::DeleteRequest {
                id: id.to_string(),
                ..Default::default()
            })
            .unwrap();
    }

    #[test]
    fn test_cri_task() -> Result<()> {
        // Currently the relationship between the "base" container and the "instances" are pretty weak.
        // When a cri sandbox is specified we just assume it's the sandbox container and treat it as such by not actually running the code (which is going to be wasm).
        let (etx, _erx) = channel();
        let exit_signal = Arc::new(ExitSignal::default());
        let local = Arc::new(Local::<Nop>::new(
            (),
            etx,
            exit_signal,
            "test_namespace".into(),
            "/test/address".into(),
        ));

        let mut _wrapped = LocalWithDescrutor::new(local.clone());

        let temp = tempdir().unwrap();
        let dir = temp.path();
        let sandbox_id = "test-cri-task".to_string();
        create_bundle(dir, Some(with_cri_sandbox(None, sandbox_id.clone())))?;

        local.task_create(api::CreateTaskRequest {
            id: "testbase".to_string(),
            bundle: dir.to_str().unwrap().to_string(),
            ..Default::default()
        })?;

        let state = local.task_state(api::StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status(), Status::CREATED);

        // A little janky since this is internal data, but check that this is seen as a sandbox container
        let i = local.get_instance("testbase")?;
        assert!(matches!(i.instance, InstanceOption::Nop(_)));

        local.task_start(api::StartRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })?;

        let state = local.task_state(api::StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status(), Status::RUNNING);

        let ll = local.clone();
        let (base_tx, base_rx) = channel();
        thread::spawn(move || {
            let resp = ll.task_wait(api::WaitRequest {
                id: "testbase".to_string(),
                ..Default::default()
            });
            base_tx.send(resp).unwrap();
        });
        base_rx.try_recv().unwrap_err();

        let temp2 = tempdir().unwrap();
        let dir2 = temp2.path();
        create_bundle(dir2, Some(with_cri_sandbox(None, sandbox_id)))?;

        local.task_create(api::CreateTaskRequest {
            id: "testinstance".to_string(),
            bundle: dir2.to_str().unwrap().to_string(),
            ..Default::default()
        })?;

        let state = local.task_state(api::StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status(), Status::CREATED);

        // again, this is janky since it is internal data, but check that this is seen as a "real" container.
        // this is the inverse of the above test case.
        let i = local.get_instance("testinstance")?;
        assert!(matches!(i.instance, InstanceOption::Instance(_)));

        local.task_start(api::StartRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })?;

        let state = local.task_state(api::StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status(), Status::RUNNING);

        let stats = local.task_stats(api::StatsRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })?;
        assert!(stats.has_stats());

        let ll = local.clone();
        let (instance_tx, instance_rx) = channel();
        std::thread::spawn(move || {
            let resp = ll.task_wait(api::WaitRequest {
                id: "testinstance".to_string(),
                ..Default::default()
            });
            instance_tx.send(resp).unwrap();
        });
        instance_rx.try_recv().unwrap_err();

        local.task_kill(api::KillRequest {
            id: "testinstance".to_string(),
            signal: 9,
            ..Default::default()
        })?;

        instance_rx.recv_timeout(Duration::from_secs(5)).unwrap()?;

        let state = local.task_state(api::StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status(), Status::STOPPED);
        local.task_delete(api::DeleteRequest {
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
        assert_eq!(state.status(), Status::RUNNING);

        local.task_kill(api::KillRequest {
            id: "testbase".to_string(),
            signal: 9,
            ..Default::default()
        })?;

        base_rx.recv_timeout(Duration::from_secs(5)).unwrap()?;
        let state = local.task_state(api::StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status(), Status::STOPPED);

        local.task_delete(api::DeleteRequest {
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
            (),
            etx,
            exit_signal,
            "test_namespace".into(),
            "/test/address".into(),
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

        local.task_create(api::CreateTaskRequest {
            id: "test".to_string(),
            bundle: dir.to_str().unwrap().to_string(),
            ..Default::default()
        })?;

        match local
            .task_create(api::CreateTaskRequest {
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

        assert_eq!(state.status(), Status::CREATED);

        local.task_start(api::StartRequest {
            id: "test".to_string(),
            ..Default::default()
        })?;

        let state = local.task_state(api::StateRequest {
            id: "test".to_string(),
            ..Default::default()
        })?;

        assert_eq!(state.status(), Status::RUNNING);

        let (tx, rx) = channel();
        let ll = local.clone();
        thread::spawn(move || {
            let resp = ll.task_wait(api::WaitRequest {
                id: "test".to_string(),
                ..Default::default()
            });
            tx.send(resp).unwrap();
        });

        rx.try_recv().unwrap_err();

        let res = local.task_stats(api::StatsRequest {
            id: "test".to_string(),
            ..Default::default()
        })?;
        assert!(res.has_stats());

        local.task_kill(api::KillRequest {
            id: "test".to_string(),
            signal: 9,
            ..Default::default()
        })?;

        rx.recv_timeout(Duration::from_secs(5)).unwrap()?;

        let state = local.task_state(api::StateRequest {
            id: "test".to_string(),
            ..Default::default()
        })?;
        assert_eq!(state.status(), Status::STOPPED);

        local.task_delete(api::DeleteRequest {
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

impl<T: Instance + Send + Sync> Local<T> {
    /// Creates a new local task service.
    pub fn new(
        engine: T::Engine,
        tx: Sender<(String, Box<dyn MessageDyn>)>,
        exit: Arc<ExitSignal>,
        namespace: String,
        containerd_address: String,
    ) -> Self {
        Self {
            // Note: engine.clone() is a shallow clone, is really cheap to do, and is safe to pass around.
            engine,
            instances: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(Mutex::new(tx)),
            exit,
            namespace,
            containerd_address,
        }
    }

    fn new_base(&self, id: String) -> InstanceData<T> {
        let cfg = InstanceConfig::new(
            self.engine.clone(),
            self.namespace.clone(),
            self.containerd_address.clone(),
        );
        InstanceData {
            instance: InstanceOption::Nop(Nop::new(id, None).unwrap()),
            cfg,
            pid: RwLock::new(None),
            state: Arc::new(RwLock::new(TaskState::Created)),
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

    fn task_create(&self, req: api::CreateTaskRequest) -> Result<api::CreateTaskResponse> {
        if !req.checkpoint().is_empty() || !req.parent_checkpoint().is_empty() {
            return Err(ShimError::Unimplemented("checkpoint is not supported".to_string()).into());
        }

        if req.terminal {
            return Err(Error::InvalidArgument(
                "terminal is not supported".to_string(),
            ));
        }

        if self.instance_exists(req.id()) {
            return Err(Error::AlreadyExists(req.id));
        }

        let mut spec = Spec::load(
            Path::new(req.bundle())
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
                        container_id: req.id,
                        bundle: req.bundle,
                        rootfs: req.rootfs,
                        io: MessageField::some(TaskIO {
                            stdin: req.stdin,
                            stdout: req.stdout,
                            stderr: req.stderr,
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

        spec.canonicalize_rootfs(req.bundle()).map_err(|err| {
            ShimError::InvalidArgument(format!("could not canonicalize rootfs: {}", err))
        })?;
        let rootfs = spec
            .root()
            .as_ref()
            .ok_or_else(|| Error::InvalidArgument("rootfs is not set in runtime spec".to_string()))?
            .path();
        let mut mkdir = DirBuilder::new();
        mkdir.recursive(true);
        #[cfg(unix)]
        mkdir.mode(0o755);
        if mkdir.create(rootfs).is_ok() { /* ignore */ }

        let rootfs_mounts = req.rootfs().to_vec();
        if !rootfs_mounts.is_empty() {
            for m in rootfs_mounts {
                let mount_type = m.type_().none_if(|&x| x.is_empty());
                let source = m.source.as_str().none_if(|&x| x.is_empty());

                #[cfg(unix)]
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
                        fs::create_dir_all(parent.unwrap()).map_err(|err| {
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
            if let Some(os) = opts {
                for o in os {
                    newopts.push(o.to_string());
                }
            }

            let mut typ = m.typ().as_deref();
            if typ.is_some() && typ.unwrap() == "bind" {
                typ = None;
                newopts.push("rbind".to_string());
            }

            #[cfg(unix)]
            mount_rootfs(typ, source, &newopts, &rootfs_target).map_err(|err| {
                ShimError::Other(format!(
                    "error mounting {} to {} as {}: {}",
                    source.unwrap_or_default(),
                    rootfs_target.to_str().unwrap(),
                    m.typ().as_deref().unwrap_or("none"),
                    err
                ))
            })?;
        }

        let engine = self.engine.clone();
        let mut builder = InstanceConfig::new(
            engine,
            self.namespace.clone(),
            self.containerd_address.clone(),
        );
        builder
            .set_stdin(req.stdin().to_string())
            .set_stdout(req.stdout().to_string())
            .set_stderr(req.stderr().to_string())
            .set_bundle(req.bundle().to_string());
        self.instances.write().unwrap().insert(
            req.id().to_string(),
            Arc::new(InstanceData {
                instance: InstanceOption::Instance(T::new(req.id().to_string(), Some(&builder))?),
                cfg: builder,
                pid: RwLock::new(None),
                state: Arc::new(RwLock::new(TaskState::Created)),
            }),
        );

        self.send_event(TaskCreate {
            container_id: req.id().into(),
            bundle: req.bundle().into(),
            rootfs: req.rootfs().into(),
            io: MessageField::some(TaskIO {
                stdin: req.stdin().into(),
                stdout: req.stdout().into(),
                stderr: req.stderr().into(),
                ..Default::default()
            }),
            ..Default::default()
        });

        debug!("create done");

        // Per the spec, the prestart hook must be called as part of the create operation
        debug!("call prehook before the start");
        oci::setup_prestart_hooks(spec.hooks())?;

        Ok(api::CreateTaskResponse {
            pid: std::process::id(),
            ..Default::default()
        })
    }

    fn task_start(&self, req: api::StartRequest) -> Result<api::StartResponse> {
        if req.exec_id().is_empty().not() {
            return Err(ShimError::Unimplemented("exec is not supported".to_string()).into());
        }

        let i = self.get_instance(req.id())?;
        let pid = i.start()?;

        let mut pid_w = i.pid.write().unwrap();
        *pid_w = Some(pid);
        drop(pid_w);

        self.send_event(TaskStart {
            container_id: req.id().into(),
            pid,
            ..Default::default()
        });

        let sender = self.events.clone();
        let id = req.id().to_string();

        thread::Builder::new()
            .name(format!("{}-wait", req.id()))
            .spawn(move || {
                let exit_code = i.wait();

                let timestamp = new_timestamp().unwrap();
                let event = TaskExit {
                    container_id: id.clone(),
                    exit_status: exit_code.0,
                    exited_at: MessageField::some(timestamp),
                    pid,
                    id,
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
            .context("could not spawn thread to wait exit")
            .map_err(Error::from)?;

        debug!("started: {:?}", req);

        Ok(api::StartResponse {
            pid,
            ..Default::default()
        })
    }

    fn task_kill(&self, req: api::KillRequest) -> Result<()> {
        if req.exec_id().is_empty().not() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()));
        }
        self.get_instance(req.id())?.kill(req.signal())?;
        Ok(())
    }

    fn task_delete(&self, req: api::DeleteRequest) -> Result<api::DeleteResponse> {
        if req.exec_id().is_empty().not() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()));
        }

        let i = self.get_instance(req.id())?;

        i.delete()?;

        let pid = i.pid.read().unwrap().unwrap_or_default();

        let mut event = TaskDelete {
            container_id: req.id().into(),
            pid,
            ..Default::default()
        };

        let mut resp = api::DeleteResponse {
            pid,
            ..Default::default()
        };

        if let Some(ec) = i.wait_timeout(Duration::ZERO) {
            event.exit_status = ec.0;
            resp.exit_status = ec.0;

            let mut ts = Timestamp::new();
            ts.seconds = ec.1.timestamp();
            ts.nanos = ec.1.timestamp_subsec_nanos() as i32;

            let timestamp = new_timestamp()?;
            event.set_exited_at(timestamp.clone());
            resp.set_exited_at(timestamp);
        }

        self.instances.write().unwrap().remove(req.id());

        self.send_event(event);
        Ok(resp)
    }

    fn task_wait(&self, req: api::WaitRequest) -> Result<api::WaitResponse> {
        if req.exec_id().is_empty().not() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()));
        }

        let i = self.get_instance(req.id())?;

        let code = i.wait();
        debug!("wait done: {:?}", req);

        let mut timestamp = Timestamp::new();
        timestamp.seconds = code.1.timestamp();
        timestamp.nanos = code.1.timestamp_subsec_nanos() as i32;

        let mut wr = api::WaitResponse {
            exit_status: code.0,
            ..Default::default()
        };
        wr.set_exited_at(timestamp);
        Ok(wr)
    }

    fn task_state(&self, req: api::StateRequest) -> Result<api::StateResponse> {
        if req.exec_id().is_empty().not() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()));
        }

        let i = self.get_instance(req.id())?;
        let mut state = api::StateResponse {
            bundle: i.cfg.get_bundle().unwrap_or_default(),
            stdin: i.cfg.get_stdin().unwrap_or_default(),
            stdout: i.cfg.get_stdout().unwrap_or_default(),
            stderr: i.cfg.get_stderr().unwrap_or_default(),
            ..Default::default()
        };

        let pid_lock = i.pid.read().unwrap();
        let pid = *pid_lock;
        if pid.is_none() {
            state.set_status(Status::CREATED);
            return Ok(state);
        }
        drop(pid_lock);

        state.set_pid(pid.unwrap());

        if let Some(c) = i.wait_timeout(Duration::ZERO) {
            state.set_status(Status::STOPPED);
            let ec = c;
            state.exit_status = ec.0;

            let mut timestamp = Timestamp::new();
            timestamp.seconds = ec.1.timestamp();
            timestamp.nanos = ec.1.timestamp_subsec_nanos() as i32;
            state.set_exited_at(timestamp);
        } else {
            state.set_status(Status::RUNNING);
        }
        Ok(state)
    }

    fn task_stats(&self, req: StatsRequest) -> Result<StatsResponse> {
        let i = self.get_instance(req.id())?;
        let pid_lock = i.pid.read().unwrap();
        let pid = *pid_lock;
        if pid.is_none() {
            return Err(Error::InvalidArgument("task is not running".to_string()));
        }

        let metrics = get_metrics(pid.unwrap())?;

        let mut stats = StatsResponse {
            ..Default::default()
        };
        stats.set_stats(metrics);
        Ok(stats)
    }
}

impl<T: Instance + Sync + Send> SandboxService for Local<T> {
    type Instance = T;
    fn new(
        namespace: String,
        containerd_address: String,
        _id: String,
        engine: T::Engine,
        publisher: RemotePublisher,
    ) -> Self {
        let (tx, rx) = channel::<(String, Box<dyn MessageDyn>)>();
        forward_events(namespace.clone(), publisher, rx);
        Local::<T>::new(
            engine,
            tx.clone(),
            Arc::new(ExitSignal::default()),
            namespace,
            containerd_address,
        )
    }
}

impl<T: Instance + Sync + Send> Task for Local<T> {
    fn create(
        &self,
        _ctx: &TtrpcContext,
        req: api::CreateTaskRequest,
    ) -> TtrpcResult<api::CreateTaskResponse> {
        debug!("create: {:?}", req);
        let resp = self.task_create(req)?;
        Ok(resp)
    }

    fn start(
        &self,
        _ctx: &::ttrpc::TtrpcContext,
        req: api::StartRequest,
    ) -> TtrpcResult<api::StartResponse> {
        debug!("start: {:?}", req);
        let resp = self.task_start(req)?;
        Ok(resp)
    }

    fn kill(&self, _ctx: &TtrpcContext, req: api::KillRequest) -> TtrpcResult<api::Empty> {
        debug!("kill: {:?}", req);
        self.task_kill(req)?;
        Ok(api::Empty::new())
    }

    fn delete(
        &self,
        _ctx: &TtrpcContext,
        req: api::DeleteRequest,
    ) -> TtrpcResult<api::DeleteResponse> {
        debug!("delete: {:?}", req);
        let resp = self.task_delete(req)?;
        Ok(resp)
    }

    fn wait(&self, _ctx: &TtrpcContext, req: api::WaitRequest) -> TtrpcResult<api::WaitResponse> {
        debug!("wait: {:?}", req);
        let resp = self.task_wait(req)?;
        Ok(resp)
    }

    fn connect(
        &self,
        _ctx: &TtrpcContext,
        req: api::ConnectRequest,
    ) -> TtrpcResult<api::ConnectResponse> {
        debug!("connect: {:?}", req);

        let i = self.get_instance(req.id())?;
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

    fn stats(
        &self,
        _ctx: &::ttrpc::TtrpcContext,
        req: StatsRequest,
    ) -> ::ttrpc::Result<StatsResponse> {
        log::info!("stats: {:?}", req);
        let resp = self.task_stats(req)?;
        Ok(resp)
    }
}

/// Cli implements the containerd-shim cli interface using `Local<T>` as the task service.
pub struct Cli<T: Instance + Sync + Send> {
    pub engine: T::Engine,
    namespace: String,
    containerd_address: String,
    phantom: std::marker::PhantomData<T>,
    exit: Arc<ExitSignal>,
    _id: String,
}

impl<I> shim::Shim for Cli<I>
where
    I: Instance + Sync + Send,
    <I as Instance>::Engine: Default,
{
    type T = Local<I>;

    fn new(_runtime_id: &str, args: &Flags, _config: &mut shim::Config) -> Self {
        Cli {
            engine: Default::default(),
            phantom: std::marker::PhantomData,
            namespace: args.namespace.to_string(),
            containerd_address: args.address.clone(),
            exit: Arc::new(ExitSignal::default()),
            _id: args.id.to_string(),
        }
    }

    fn start_shim(&mut self, opts: containerd_shim::StartOpts) -> shim::Result<String> {
        let dir = current_dir().map_err(|err| ShimError::Other(err.to_string()))?;
        let spec = Spec::load(dir.join("config.json").to_str().unwrap()).map_err(|err| {
            shim::Error::InvalidArgument(format!("error loading runtime spec: {}", err))
        })?;

        let default = HashMap::new() as HashMap<String, String>;
        let annotations = spec.annotations().as_ref().unwrap_or(&default);

        let id = opts.id.clone();

        let grouping = annotations
            .get("io.kubernetes.cri.sandbox-id")
            .unwrap_or(&id)
            .to_string();

        setup_namespaces(&spec)
            .map_err(|e| shim::Error::Other(format!("failed to setup namespaces: {}", e)))?;

        #[cfg(unix)]
        mount::<str, Path, str, str>(
            None,
            "/".as_ref(),
            None,
            MsFlags::MS_REC | MsFlags::MS_SLAVE,
            None,
        )
        .map_err(|err| {
            shim::Error::Other(format!("failed to remount rootfs as secondary: {}", err))
        })?;

        let envs = vec![] as Vec<(&str, &str)>;
        let (_child, address) = shim::spawn(opts, &grouping, envs)?;

        write_address(&address)?;

        Ok(address)
    }

    fn wait(&mut self) {
        self.exit.wait();
    }

    fn create_task_service(&self, publisher: RemotePublisher) -> Self::T {
        let (tx, rx) = channel::<(String, Box<dyn MessageDyn>)>();
        forward_events(self.namespace.to_string(), publisher, rx);
        Local::<I>::new(
            self.engine.clone(),
            tx.clone(),
            self.exit.clone(),
            self.namespace.clone(),
            self.containerd_address.clone(),
        )
    }

    fn delete_shim(&mut self) -> shim::Result<api::DeleteResponse> {
        let timestamp = new_timestamp()?;
        Ok(api::DeleteResponse {
            exit_status: 137,
            exited_at: MessageField::some(timestamp),
            ..Default::default()
        })
    }
}

fn forward_events(
    namespace: String,
    publisher: RemotePublisher,
    events: Receiver<(String, Box<dyn MessageDyn>)>,
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
