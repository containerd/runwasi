use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use shim_instrument::shim_instrument as instrument;

use crate::sandbox::instance::Nop;
use crate::sandbox::shim::instance_option::InstanceOption;
use crate::sandbox::shim::task_state::TaskState;
use crate::sandbox::{Instance, InstanceConfig, Result};

pub(super) struct InstanceData<T: Instance> {
    pub instance: InstanceOption<T>,
    cfg: InstanceConfig<T::Engine>,
    pid: OnceLock<u32>,
    state: Arc<RwLock<TaskState>>,
}

impl<T: Instance> InstanceData<T> {
    #[instrument(skip_all, level = "Info")]
    pub fn new_instance(id: impl AsRef<str>, cfg: InstanceConfig<T::Engine>) -> Result<Self> {
        let id = id.as_ref().to_string();
        let instance = InstanceOption::Instance(T::new(id, Some(&cfg))?);
        Ok(Self {
            instance,
            cfg,
            pid: OnceLock::default(),
            state: Arc::new(RwLock::new(TaskState::Created)),
        })
    }

    #[instrument(skip_all, level = "Info")]
    pub fn new_base(id: impl AsRef<str>, cfg: InstanceConfig<T::Engine>) -> Result<Self> {
        let id = id.as_ref().to_string();
        let instance = InstanceOption::Nop(Nop::new(id, None)?);
        Ok(Self {
            instance,
            cfg,
            pid: OnceLock::default(),
            state: Arc::new(RwLock::new(TaskState::Created)),
        })
    }

    #[instrument(skip_all, level = "Info")]
    pub fn pid(&self) -> Option<u32> {
        self.pid.get().copied()
    }

    #[instrument(skip_all, level = "Info")]
    pub fn config(&self) -> &InstanceConfig<T::Engine> {
        &self.cfg
    }

    #[instrument(skip_all, level = "Info")]
    pub fn start(&self) -> Result<u32> {
        let mut s = self.state.write().unwrap();
        s.start()?;

        let res = self.instance.start();

        // These state transitions are always `Ok(())` because
        // we hold the lock since `s.start()`
        let _ = match res {
            Ok(pid) => {
                let _ = self.pid.set(pid);
                s.started()
            }
            Err(_) => s.stop(),
        };

        res
    }

    #[instrument(skip_all, level = "Info")]
    pub fn kill(&self, signal: u32) -> Result<()> {
        let mut s = self.state.write().unwrap();
        s.kill()?;

        self.instance.kill(signal)
    }

    #[instrument(skip_all, level = "Info")]
    pub fn delete(&self) -> Result<()> {
        let mut s = self.state.write().unwrap();
        s.delete()?;

        let res = self.instance.delete();

        if res.is_err() {
            // Always `Ok(())` because we hold the lock since `s.delete()`
            let _ = s.stop();
        }

        res
    }

    #[instrument(skip_all, level = "Info")]
    pub fn wait(&self) -> (u32, DateTime<Utc>) {
        let res = self.instance.wait();
        let mut s = self.state.write().unwrap();
        *s = TaskState::Exited;
        res
    }

    #[instrument(skip_all, level = "Info")]
    pub fn wait_timeout(&self, t: impl Into<Option<Duration>>) -> Option<(u32, DateTime<Utc>)> {
        let res = self.instance.wait_timeout(t);
        if res.is_some() {
            let mut s = self.state.write().unwrap();
            *s = TaskState::Exited;
        }
        res
    }
}
