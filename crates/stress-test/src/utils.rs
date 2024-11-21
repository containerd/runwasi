use std::future::{pending, Future};
use std::hash::{DefaultHasher, Hash, Hasher as _};
use std::sync::Arc;

use anyhow::{bail, Result};
use nix::sys::signal::kill;
use nix::sys::signal::Signal::SIGKILL;
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::Pid;
use tokio::select;
use tokio::signal::ctrl_c;
use tokio::sync::Notify;
use tokio::time::{sleep, Duration};

pub fn hash(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
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

pub trait TryFutureEx<E> {
    fn or_ctrl_c(self) -> impl Future<Output = Result<E>> + Send;
    fn with_watchdog(
        self,
        t: Duration,
        ping: Arc<Notify>,
    ) -> impl Future<Output = Result<E>> + Send;
}

impl<E: Default, T: Future<Output = Result<E>> + Send> TryFutureEx<E> for T {
    async fn or_ctrl_c(self) -> Result<E> {
        select! {
            val = self => { val },
            _ = ctrl_c() => {
                println!();
                bail!("Terminated");
            }
        }
    }

    async fn with_watchdog(self, t: Duration, ping: Arc<Notify>) -> Result<E> {
        let timeout = |t: Duration| async move {
            if t.is_zero() {
                pending().await
            } else {
                sleep(t).await
            }
        };

        let mut timer = timeout(t.clone());

        let fut = self;
        tokio::pin!(fut);

        loop {
            select! {
                val = &mut fut => { return val; },
                _ = ping.notified() => { timer = timeout(t.clone()); }
                _ = timer => { bail!("Timeout"); }
            }
        }
    }
}

pub trait DropIf {
    fn drop_if(&mut self, cond: bool);
}

impl<T> DropIf for Option<T> {
    fn drop_if(&mut self, cond: bool) {
        if cond {
            self.take();
        }
    }
}
