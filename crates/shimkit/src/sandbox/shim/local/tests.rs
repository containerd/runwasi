use std::fs::{File, create_dir};
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use containerd_shim::api::Status;
use containerd_shim::event::Event;
use protobuf::{MessageDyn, SpecialFields};
use serde_json as json;
use tempfile::tempdir;
use tokio::sync::mpsc::{UnboundedSender as Sender, unbounded_channel as channel};
use tokio_async_drop::tokio_async_drop;

use super::*;
use crate::sandbox::shim::events::EventSender;
use crate::sandbox::sync::WaitableCell;

/// This is used for the tests and is a no-op instance implementation.
pub struct InstanceStub {
    /// Since we are faking the container, we need to keep track of the "exit" code/time
    /// We'll just mark it as exited when kill is called.
    exit_code: WaitableCell<(u32, DateTime<Utc>)>,
}

impl Instance for InstanceStub {
    async fn new(_id: String, _cfg: &InstanceConfig) -> Result<Self, Error> {
        Ok(InstanceStub {
            exit_code: WaitableCell::new(),
        })
    }
    async fn start(&self) -> Result<u32, Error> {
        Ok(std::process::id())
    }
    async fn kill(&self, _signal: u32) -> Result<(), Error> {
        let _ = self.exit_code.set((1, Utc::now()));
        Ok(())
    }
    async fn delete(&self) -> Result<(), Error> {
        Ok(())
    }
    async fn wait(&self) -> (u32, DateTime<Utc>) {
        *self.exit_code.wait().await
    }
}

struct LocalWithDestructor<T: Instance + Send + Sync, E: EventSender> {
    local: Arc<Local<T, E>>,
}

impl<T: Instance + Send + Sync, E: EventSender> LocalWithDestructor<T, E> {
    fn new(local: Arc<Local<T, E>>) -> Self {
        Self { local }
    }
}

impl EventSender for Sender<(String, Box<dyn MessageDyn>)> {
    fn send(&self, event: impl Event) {
        let _ = self.send((event.topic(), Box::new(event)));
    }
}

impl<T: Instance + Send + Sync, E: EventSender> Drop for LocalWithDestructor<T, E> {
    fn drop(&mut self) {
        tokio_async_drop!({
            let instances = self.local.instances.write().await;
            for (_, instance) in instances.iter() {
                let _ = instance.kill(9).await;
                let _ = instance.delete().await;
            }
        })
    }
}

fn with_cri_sandbox(spec: Option<Spec>, id: String) -> Spec {
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

fn create_bundle(dir: &std::path::Path, spec: Option<Spec>) -> Result<()> {
    create_dir(dir.join("rootfs"))?;

    let s = spec.unwrap_or_default();

    json::to_writer(File::create(dir.join("config.json"))?, &s)
        .context("could not write config.json")?;
    Ok(())
}

// Use a multi threaded runtime because LocalWithDestructor needs
// it to run its async drop.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_delete_after_create() -> anyhow::Result<()> {
    let dir = tempdir().unwrap();
    let id = "test-delete-after-create";
    create_bundle(dir.path(), None).unwrap();

    let (tx, _rx) = channel();
    let local = Arc::new(Local::<InstanceStub, _>::new(
        tx,
        WaitableCell::new(),
        "test_namespace",
        "/test/address",
    ));
    let mut _wrapped = LocalWithDestructor::new(local.clone());

    local
        .task_create(CreateTaskRequest {
            id: id.to_string(),
            bundle: dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        })
        .await?;

    local
        .task_delete(DeleteRequest {
            id: id.to_string(),
            ..Default::default()
        })
        .await?;

    Ok(())
}

// Use a multi threaded runtime because LocalWithDestructor needs
// it to run its async drop.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cri_task() -> Result<()> {
    // Currently the relationship between the "base" container and the "instances" are pretty weak.
    // When a cri sandbox is specified we just assume it's the sandbox container and treat it as such by not actually running the code (which is going to be wasm).
    let (etx, _erx) = channel();
    let exit_signal = WaitableCell::new();
    let local = Arc::new(Local::<InstanceStub, _>::new(
        etx,
        exit_signal,
        "test_namespace",
        "/test/address",
    ));

    let mut _wrapped = LocalWithDestructor::new(local.clone());

    let temp = tempdir().unwrap();
    let dir = temp.path();
    let sandbox_id = "test-cri-task".to_string();
    create_bundle(dir, Some(with_cri_sandbox(None, sandbox_id.clone())))?;

    local
        .task_create(CreateTaskRequest {
            id: "testbase".to_string(),
            bundle: dir.to_str().unwrap().to_string(),
            ..Default::default()
        })
        .await?;

    let state = local
        .task_state(StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })
        .await?;
    assert_eq!(state.status(), Status::CREATED);

    // make sure that the instance exists
    let _i = local.get_instance("testbase").await?;

    local
        .task_start(StartRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })
        .await?;

    let state = local
        .task_state(StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })
        .await?;
    assert_eq!(state.status(), Status::RUNNING);

    let ll = local.clone();
    let (base_tx, mut base_rx) = channel();
    tokio::spawn(async move {
        let resp = ll
            .task_wait(WaitRequest {
                id: "testbase".to_string(),
                ..Default::default()
            })
            .await;
        base_tx.send(resp).unwrap();
    });
    base_rx.try_recv().unwrap_err();

    let temp2 = tempdir().unwrap();
    let dir2 = temp2.path();
    create_bundle(dir2, Some(with_cri_sandbox(None, sandbox_id)))?;

    local
        .task_create(CreateTaskRequest {
            id: "testinstance".to_string(),
            bundle: dir2.to_str().unwrap().to_string(),
            ..Default::default()
        })
        .await?;

    let state = local
        .task_state(StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })
        .await?;
    assert_eq!(state.status(), Status::CREATED);

    // make sure that the instance exists
    let _i = local.get_instance("testinstance").await?;

    local
        .task_start(StartRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })
        .await?;

    let state = local
        .task_state(StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })
        .await?;
    assert_eq!(state.status(), Status::RUNNING);

    let stats = local
        .task_stats(StatsRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })
        .await?;
    assert!(stats.has_stats());

    let ll = local.clone();
    let (instance_tx, mut instance_rx) = channel();
    tokio::spawn(async move {
        let resp = ll
            .task_wait(WaitRequest {
                id: "testinstance".to_string(),
                ..Default::default()
            })
            .await;
        instance_tx.send(resp).unwrap();
    });
    instance_rx.try_recv().unwrap_err();

    local
        .task_kill(KillRequest {
            id: "testinstance".to_string(),
            signal: 9,
            ..Default::default()
        })
        .await?;

    instance_rx
        .recv()
        .with_timeout(Duration::from_secs(50))
        .await
        .flatten()
        .unwrap()?;

    let state = local
        .task_state(StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })
        .await?;
    assert_eq!(state.status(), Status::STOPPED);
    local
        .task_delete(DeleteRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })
        .await?;

    match local
        .task_state(StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })
        .await
        .unwrap_err()
    {
        Error::NotFound(_) => {}
        e => return Err(e),
    }

    base_rx.try_recv().unwrap_err();
    let state = local
        .task_state(StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })
        .await?;
    assert_eq!(state.status(), Status::RUNNING);

    local
        .task_kill(KillRequest {
            id: "testbase".to_string(),
            signal: 9,
            ..Default::default()
        })
        .await?;

    base_rx
        .recv()
        .with_timeout(Duration::from_secs(5))
        .await
        .flatten()
        .unwrap()?;
    let state = local
        .task_state(StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })
        .await?;
    assert_eq!(state.status(), Status::STOPPED);

    local
        .task_delete(DeleteRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })
        .await?;
    match local
        .task_state(StateRequest {
            id: "testbase".to_string(),
            ..Default::default()
        })
        .await
        .unwrap_err()
    {
        Error::NotFound(_) => {}
        e => return Err(e),
    }

    Ok(())
}

// Use a multi threaded runtime because LocalWithDestructor needs
// it to run its async drop.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_task_lifecycle() -> Result<()> {
    let (etx, _erx) = channel(); // TODO: check events
    let exit_signal = WaitableCell::new();
    let local = Arc::new(Local::<InstanceStub, _>::new(
        etx,
        exit_signal,
        "test_namespace",
        "/test/address",
    ));

    let mut _wrapped = LocalWithDestructor::new(local.clone());

    let temp = tempdir().unwrap();
    let dir = temp.path();
    create_bundle(dir, None)?;

    match local
        .task_state(StateRequest {
            id: "test".to_string(),
            ..Default::default()
        })
        .await
        .unwrap_err()
    {
        Error::NotFound(_) => {}
        e => return Err(e),
    }

    local
        .task_create(CreateTaskRequest {
            id: "test".to_string(),
            bundle: dir.to_str().unwrap().to_string(),
            ..Default::default()
        })
        .await?;

    match local
        .task_create(CreateTaskRequest {
            id: "test".to_string(),
            bundle: dir.to_str().unwrap().to_string(),
            ..Default::default()
        })
        .await
        .unwrap_err()
    {
        Error::AlreadyExists(_) => {}
        e => return Err(e),
    }

    let state = local
        .task_state(StateRequest {
            id: "test".to_string(),
            ..Default::default()
        })
        .await?;

    assert_eq!(state.status(), Status::CREATED);

    local
        .task_start(StartRequest {
            id: "test".to_string(),
            ..Default::default()
        })
        .await?;

    let state = local
        .task_state(StateRequest {
            id: "test".to_string(),
            ..Default::default()
        })
        .await?;

    assert_eq!(state.status(), Status::RUNNING);

    let (tx, mut rx) = channel();
    let ll = local.clone();
    tokio::spawn(async move {
        let resp = ll
            .task_wait(WaitRequest {
                id: "test".to_string(),
                ..Default::default()
            })
            .await;
        tx.send(resp).unwrap();
    });

    rx.try_recv().unwrap_err();

    let res = local
        .task_stats(StatsRequest {
            id: "test".to_string(),
            ..Default::default()
        })
        .await?;
    assert!(res.has_stats());

    local
        .task_kill(KillRequest {
            id: "test".to_string(),
            signal: 9,
            ..Default::default()
        })
        .await?;

    rx.recv()
        .with_timeout(Duration::from_secs(5))
        .await
        .flatten()
        .unwrap()?;

    let state = local
        .task_state(StateRequest {
            id: "test".to_string(),
            ..Default::default()
        })
        .await?;
    assert_eq!(state.status(), Status::STOPPED);

    local
        .task_delete(DeleteRequest {
            id: "test".to_string(),
            ..Default::default()
        })
        .await?;

    match local
        .task_state(StateRequest {
            id: "test".to_string(),
            ..Default::default()
        })
        .await
        .unwrap_err()
    {
        Error::NotFound(_) => {}
        e => return Err(e),
    }

    Ok(())
}

#[test]
fn test_default_runtime_options() -> Result<()> {
    let options: Option<&Any> = None;

    let config = Config::get_from_options(options).unwrap();

    assert_eq!(config.systemd_cgroup, false);

    Ok(())
}

#[test]
fn test_custom_runtime_options() -> Result<()> {
    let options = Options {
        type_url: "runtimeoptions.v1.Options".to_string(),
        config_path: "".to_string(),
        config_body: "SystemdCgroup = true\n".to_string(),
    };
    let req = CreateTaskRequest {
        options: Some(Any {
            type_url: options.type_url.clone(),
            value: options.encode_to_vec(),
            special_fields: SpecialFields::default(),
        })
        .into(),
        ..Default::default()
    };

    let config = Config::get_from_options(req.options.as_ref()).unwrap();

    assert_eq!(config.systemd_cgroup, true);

    Ok(())
}
