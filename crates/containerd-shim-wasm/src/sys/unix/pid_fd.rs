use std::os::fd::{AsFd, FromRawFd as _, OwnedFd, RawFd};

use libc::pid_t;
use nix::sys::wait::{waitid, Id, WaitPidFlag, WaitStatus};
use tokio::io::unix::AsyncFd;

pub(super) struct PidFd {
    fd: OwnedFd,
}

impl PidFd {
    pub(super) fn new(pid: impl Into<pid_t>) -> anyhow::Result<Self> {
        use libc::{syscall, SYS_pidfd_open, PIDFD_NONBLOCK};
        let pidfd = unsafe { syscall(SYS_pidfd_open, pid.into(), PIDFD_NONBLOCK) };
        if pidfd == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(pidfd as RawFd) };
        Ok(Self { fd })
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
            )? {
                WaitStatus::StillAlive => {
                    let _ = fd.readable().await?;
                }
                status => {
                    return Ok(status);
                }
            }
        }
    }
}
