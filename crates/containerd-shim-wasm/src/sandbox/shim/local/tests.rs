use std::fs::{create_dir, File};
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::Duration;

use anyhow::Context;
use containerd_shim::api::Status;
use containerd_shim::event::Event;
use protobuf::MessageDyn;
use serde_json as json;
use tempfile::tempdir;

use super::*;
use crate::sandbox::instance::Nop;
use crate::sandbox::shim::events::EventSender;
use crate::sandbox::shim::instance_option::InstanceOption;

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

#[test]
fn test_delete_after_create() {
    let dir = tempdir().unwrap();
    let id = "test-delete-after-create";
    create_bundle(dir.path(), None).unwrap();

    let (tx, _rx) = channel();
    let local = Arc::new(Local::<Nop, _>::new(
        (),
        tx,
        Arc::new(ExitSignal::default()),
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
        .unwrap();

    local
        .task_delete(DeleteRequest {
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
    let local = Arc::new(Local::<Nop, _>::new(
        (),
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

    local.task_create(CreateTaskRequest {
        id: "testbase".to_string(),
        bundle: dir.to_str().unwrap().to_string(),
        ..Default::default()
    })?;

    let state = local.task_state(StateRequest {
        id: "testbase".to_string(),
        ..Default::default()
    })?;
    assert_eq!(state.status(), Status::CREATED);

    // A little janky since this is internal data, but check that this is seen as a sandbox container
    let i = local.get_instance("testbase")?;
    assert!(matches!(i.instance, InstanceOption::Nop(_)));

    local.task_start(StartRequest {
        id: "testbase".to_string(),
        ..Default::default()
    })?;

    let state = local.task_state(StateRequest {
        id: "testbase".to_string(),
        ..Default::default()
    })?;
    assert_eq!(state.status(), Status::RUNNING);

    let ll = local.clone();
    let (base_tx, base_rx) = channel();
    thread::spawn(move || {
        let resp = ll.task_wait(WaitRequest {
            id: "testbase".to_string(),
            ..Default::default()
        });
        base_tx.send(resp).unwrap();
    });
    base_rx.try_recv().unwrap_err();

    let temp2 = tempdir().unwrap();
    let dir2 = temp2.path();
    create_bundle(dir2, Some(with_cri_sandbox(None, sandbox_id)))?;

    local.task_create(CreateTaskRequest {
        id: "testinstance".to_string(),
        bundle: dir2.to_str().unwrap().to_string(),
        ..Default::default()
    })?;

    let state = local.task_state(StateRequest {
        id: "testinstance".to_string(),
        ..Default::default()
    })?;
    assert_eq!(state.status(), Status::CREATED);

    // again, this is janky since it is internal data, but check that this is seen as a "real" container.
    // this is the inverse of the above test case.
    let i = local.get_instance("testinstance")?;
    assert!(matches!(i.instance, InstanceOption::Instance(_)));

    local.task_start(StartRequest {
        id: "testinstance".to_string(),
        ..Default::default()
    })?;

    let state = local.task_state(StateRequest {
        id: "testinstance".to_string(),
        ..Default::default()
    })?;
    assert_eq!(state.status(), Status::RUNNING);

    let stats = local.task_stats(StatsRequest {
        id: "testinstance".to_string(),
        ..Default::default()
    })?;
    assert!(stats.has_stats());

    let ll = local.clone();
    let (instance_tx, instance_rx) = channel();
    std::thread::spawn(move || {
        let resp = ll.task_wait(WaitRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        });
        instance_tx.send(resp).unwrap();
    });
    instance_rx.try_recv().unwrap_err();

    local.task_kill(KillRequest {
        id: "testinstance".to_string(),
        signal: 9,
        ..Default::default()
    })?;

    instance_rx.recv_timeout(Duration::from_secs(5)).unwrap()?;

    let state = local.task_state(StateRequest {
        id: "testinstance".to_string(),
        ..Default::default()
    })?;
    assert_eq!(state.status(), Status::STOPPED);
    local.task_delete(DeleteRequest {
        id: "testinstance".to_string(),
        ..Default::default()
    })?;

    match local
        .task_state(StateRequest {
            id: "testinstance".to_string(),
            ..Default::default()
        })
        .unwrap_err()
    {
        Error::NotFound(_) => {}
        e => return Err(e),
    }

    base_rx.try_recv().unwrap_err();
    let state = local.task_state(StateRequest {
        id: "testbase".to_string(),
        ..Default::default()
    })?;
    assert_eq!(state.status(), Status::RUNNING);

    local.task_kill(KillRequest {
        id: "testbase".to_string(),
        signal: 9,
        ..Default::default()
    })?;

    base_rx.recv_timeout(Duration::from_secs(5)).unwrap()?;
    let state = local.task_state(StateRequest {
        id: "testbase".to_string(),
        ..Default::default()
    })?;
    assert_eq!(state.status(), Status::STOPPED);

    local.task_delete(DeleteRequest {
        id: "testbase".to_string(),
        ..Default::default()
    })?;
    match local
        .task_state(StateRequest {
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
    let local = Arc::new(Local::<Nop, _>::new(
        (),
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
        .unwrap_err()
    {
        Error::NotFound(_) => {}
        e => return Err(e),
    }

    local.task_create(CreateTaskRequest {
        id: "test".to_string(),
        bundle: dir.to_str().unwrap().to_string(),
        ..Default::default()
    })?;

    match local
        .task_create(CreateTaskRequest {
            id: "test".to_string(),
            bundle: dir.to_str().unwrap().to_string(),
            ..Default::default()
        })
        .unwrap_err()
    {
        Error::AlreadyExists(_) => {}
        e => return Err(e),
    }

    let state = local.task_state(StateRequest {
        id: "test".to_string(),
        ..Default::default()
    })?;

    assert_eq!(state.status(), Status::CREATED);

    local.task_start(StartRequest {
        id: "test".to_string(),
        ..Default::default()
    })?;

    let state = local.task_state(StateRequest {
        id: "test".to_string(),
        ..Default::default()
    })?;

    assert_eq!(state.status(), Status::RUNNING);

    let (tx, rx) = channel();
    let ll = local.clone();
    thread::spawn(move || {
        let resp = ll.task_wait(WaitRequest {
            id: "test".to_string(),
            ..Default::default()
        });
        tx.send(resp).unwrap();
    });

    rx.try_recv().unwrap_err();

    let res = local.task_stats(StatsRequest {
        id: "test".to_string(),
        ..Default::default()
    })?;
    assert!(res.has_stats());

    local.task_kill(KillRequest {
        id: "test".to_string(),
        signal: 9,
        ..Default::default()
    })?;

    rx.recv_timeout(Duration::from_secs(5)).unwrap()?;

    let state = local.task_state(StateRequest {
        id: "test".to_string(),
        ..Default::default()
    })?;
    assert_eq!(state.status(), Status::STOPPED);

    local.task_delete(DeleteRequest {
        id: "test".to_string(),
        ..Default::default()
    })?;

    match local
        .task_state(StateRequest {
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
