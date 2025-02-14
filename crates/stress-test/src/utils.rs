use std::future::{pending, Future};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Result;
use nix::sys::signal::kill;
use nix::sys::signal::Signal::SIGKILL;
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::Pid;
use tokio::sync::Mutex;
use tokio::time::sleep;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn make_task_id() -> String {
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("shim-stress-test-{pid}-task-{n}")
}

pub async fn reap_children() -> Result<()> {
    let pid = std::process::id();
    loop {
        let list: Vec<u32> = tokio::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
            .await?
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();

        if list.is_empty() {
            return Ok(());
        }

        for pid in list {
            let pid = Pid::from_raw(pid as _);
            let _ = kill(pid, SIGKILL);
            let _ = waitpid(pid, Some(WaitPidFlag::WNOHANG));
        }
    }
}

pub async fn watchdog(timeout: Duration) {
    if timeout.is_zero() {
        pending().await
    } else {
        sleep(timeout).await
    }
}

pub struct RunOnce(Mutex<bool>);

impl RunOnce {
    pub fn new() -> Self {
        Self(Mutex::new(false))
    }

    pub async fn try_run(&self, fut: impl Future<Output = Result<()>>) -> Result<()> {
        let mut done = self.0.lock().await;
        if !*done {
            fut.await?;
            *done = true;
        }
        Ok(())
    }
}
