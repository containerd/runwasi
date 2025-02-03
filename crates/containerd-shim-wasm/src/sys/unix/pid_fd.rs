use std::os::fd::{AsFd, FromRawFd as _, OwnedFd, RawFd};

use containerd_shim::monitor::{monitor_subscribe, wait_pid, Subscription, Topic};
use libc::pid_t;
use nix::errno::Errno;
use nix::sys::wait::{waitid, Id, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use tokio::io::unix::AsyncFd;

pub(super) struct PidFd {
    fd: OwnedFd,
    pid: pid_t,
    subs: Subscription,
}

impl PidFd {
    pub(super) fn new(pid: impl Into<pid_t>) -> anyhow::Result<Self> {
        use libc::{syscall, SYS_pidfd_open, PIDFD_NONBLOCK};
        let pid = pid.into();
        let pidfd = unsafe { syscall(SYS_pidfd_open, pid, PIDFD_NONBLOCK) };
        if pidfd == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(pidfd as RawFd) };
        let subs = monitor_subscribe(Topic::Pid)?;
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
                    let status = wait_pid(self.pid, self.subs);
                    return Ok(WaitStatus::Exited(Pid::from_raw(self.pid), status));
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
}
