use std::os::fd::{AsFd, FromRawFd as _, OwnedFd, RawFd};
use std::time::Duration;

use containerd_shim::monitor::{ExitEvent, Subject, Subscription, Topic, monitor_subscribe};
use libc::pid_t;
use nix::errno::Errno;
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
use nix::unistd::Pid;
use tokio::io::unix::AsyncFd;

use crate::sandbox::async_utils::AmbientRuntime;

pub(super) struct PidFd {
    fd: OwnedFd,
    pid: pid_t,
    subs: Subscription,
}

impl PidFd {
    pub(super) async fn new(pid: impl Into<pid_t>) -> anyhow::Result<Self> {
        use libc::{PIDFD_NONBLOCK, SYS_pidfd_open, syscall};
        let pid = pid.into();
        let subs = monitor_subscribe(Topic::Pid).await?;
        let pidfd = unsafe { syscall(SYS_pidfd_open, pid, PIDFD_NONBLOCK) };
        if pidfd == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(pidfd as RawFd) };
        Ok(Self { fd, pid, subs })
    }

    pub(super) async fn wait(self) -> std::io::Result<WaitStatus> {
        let fd = AsyncFd::new(self.fd)?;
        loop {
            // Check with non-blocking waitid before awaiting on fd.
            // On some platforms, the readiness detecting mechanism relies on
            // edge-triggered notifications.
            // This means that we could miss a notification if the process exits
            // before we create the AsyncFd.
            // See https://docs.rs/tokio/latest/tokio/io/unix/struct.AsyncFd.html
            match waitid(
                Id::PIDFd(fd.as_fd()),
                WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG,
            ) {
                Ok(WaitStatus::StillAlive) => {
                    let _ = fd.readable().await?;
                }
                Ok(status) => {
                    return Ok(status);
                }
                Err(Errno::ECHILD) => {
                    // The process has already been reaped by the containerd-shim reaper.
                    // Get the status from there.
                    let status = try_wait_pid(self.pid, self.subs).await?;
                    return Ok(WaitStatus::Exited(Pid::from_raw(self.pid), status));
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
}

pub async fn try_wait_pid(pid: i32, mut s: Subscription) -> Result<i32, Errno> {
    while let Some(ExitEvent { subject, exit_code }) =
        s.rx.recv()
            .with_timeout(Duration::from_secs(2))
            .await
            .flatten()
    {
        let Subject::Pid(p) = subject else {
            continue;
        };
        if pid == p {
            return Ok(exit_code);
        }
    }
    Err(Errno::ECHILD)
}
