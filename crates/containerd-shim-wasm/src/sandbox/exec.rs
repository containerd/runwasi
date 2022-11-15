use super::cgroups::{Cgroup, Version as CgroupVersion};
use super::error::Error;
use caps::{CapSet, Capability};
use clone3::Clone3;
use libc::siginfo_t;
use log::debug;
use log::info;
use nix::sys::signal::SIGCHLD;
use nix::sys::wait::{waitid, Id as WaitID, WaitPidFlag, WaitStatus};
use nix::unistd::close;
use std::os::raw::c_int as RawFD;
use std::os::unix::io::AsRawFd;
use std::ptr;

#[derive(Clone)]
pub struct PidFD {
    fd: RawFD,
}

impl Drop for PidFD {
    fn drop(&mut self) {
        let _ = wait(self.fd, true); // don't leave zombies
        let _ = unsafe { libc::close(self.fd) };
    }
}
unsafe fn pidfd_send_signal(pidfd: RawFD, sig: i32, info: *mut siginfo_t, flags: u32) -> i64 {
    libc::syscall(libc::SYS_pidfd_send_signal, pidfd, sig, info, flags)
}

impl AsRawFd for PidFD {
    fn as_raw_fd(&self) -> RawFD {
        self.fd
    }
}

#[derive(Debug)]
pub enum Status {
    Exited(ExitStatus),
    Running,
}

impl From<Status> for ExitStatus {
    fn from(status: Status) -> Self {
        match status {
            Status::Exited(status) => status,
            Status::Running => panic!("cannot convert running process to exit status"),
        }
    }
}

fn wait(fd: RawFD, no_hang: bool) -> Result<Status, Error> {
    let no_hang = if no_hang {
        WaitPidFlag::WNOHANG
    } else {
        WaitPidFlag::empty()
    };

    let info = waitid(WaitID::PIDFd(fd), no_hang | WaitPidFlag::WEXITED)?;

    match info {
        WaitStatus::Exited(pid, status) => Ok(Status::Exited(ExitStatus {
            pid: pid.as_raw() as u32,
            status: status as u32,
        })),
        WaitStatus::Signaled(pid, sig, dumped) => {
            info!("child {} killed by signal {}, dumped: {}", pid, sig, dumped);
            Ok(Status::Exited(ExitStatus {
                pid: pid.as_raw() as u32,
                status: 128 + sig as u32,
            }))
        }
        WaitStatus::StillAlive => Ok(Status::Running),
        _ => Err(Error::Others(format!("unexpected wait status: {:?}", info))),
    }
}

impl PidFD {
    // wait for the process referred to by the pidfd to exit
    //
    // If you want more control over waiting you can use `as_raw_fd()` and call `waitid` yourself.
    pub fn wait(&self) -> Result<ExitStatus, Error> {
        let ws = wait(self.fd, false)?;
        Ok(ws.into())
    }

    pub fn is_running(&self) -> Result<bool, Error> {
        match wait(self.fd, true) {
            Ok(Status::Running) => Ok(true),
            Ok(Status::Exited(_)) => Ok(false),
            Err(Error::Errno(nix::errno::Errno::ECHILD)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    // Send the specified signal to the process referred to by the pidfd.
    pub fn kill(&self, sig: i32) -> Result<(), Error> {
        let ret = unsafe { pidfd_send_signal(self.fd, sig, ptr::null_mut(), 0) };
        if ret == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ExitStatus {
    pub pid: u32,
    pub status: u32,
}

pub enum Context {
    // Parent stores the pid of the child process and the pidfd that can be used to, for instance, wait on the child.
    Parent(u32, PidFD),
    Child,
}

pub fn has_cap_sys_admin() -> bool {
    let caps = caps::read(None, CapSet::Effective).unwrap();
    caps.contains(&Capability::CAP_SYS_ADMIN)
}

// This is is a wrapper around clone3 which allows us to pass a pidfd
// This works otherwise just like normal forking semantics:
// If this is the parent, the result will be Ok(Context::Parent(pid, pidfd)), where the pid is the pid of the new process.
// If this is the child, the result will be Ok(Context::Child).
//
// Code that runs in the child must not do things like access locks or other shared state.
// The child should not depend on other threads in the parent process since the new process becomes single threaded.
pub unsafe fn fork(cgroup: Option<&dyn Cgroup>) -> Result<Context, Error> {
    let mut builder = Clone3::default();

    let mut fd: RawFD = -1;
    builder.flag_pidfd(&mut fd);

    builder.exit_signal(SIGCHLD as u64).flag_ptrace();

    let is_root = has_cap_sys_admin();

    let mut cgfd: RawFD = -1; // Keep the fd alive until we return
    if is_root {
        if let Some(cgroup) = &cgroup {
            if cgroup.version() == CgroupVersion::V2 {
                cgfd = cgroup.open()?;
                builder.flag_into_cgroup(&cgfd).flag_newcgroup();
            }
        }
    } else {
        debug!("no CAP_SYS_ADMIN, not creating new namespaces");
    }

    let res = builder.call();
    if cgfd > -1 {
        match close(cgfd) {
            Ok(_) => {}
            Err(e) => {
                info!("failed to close cgroup fd: {}", e);
            }
        }
    }
    match res {
        Ok(tid) => match tid {
            0 => {
                if is_root {
                    if let Some(cgroup) = cgroup {
                        // With v2 we use clone_into_cgroup, so we only want to handle this for v1
                        if cgroup.version() == CgroupVersion::V1 {
                            cgroup.add_task(std::process::id()).map_err(|e| {
                                Error::Others(format!("error adding pid to cgroup: {}", e))
                            })?;
                        }
                    }
                }
                Ok(Context::Child)
            }
            _ => Ok(Context::Parent(tid as u32, PidFD { fd })),
        },
        Err(e) => Err(std::io::Error::from(e).into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::sandbox::testutil::run_test_with_sudo;

    use super::super::cgroups;
    use super::super::testutil::function;
    use super::*;
    use nix::unistd::close;
    use nix::{
        fcntl::OFlag,
        unistd::pipe2,
        unistd::{read, write},
    };
    use signal_hook::consts::SIGUSR2 as TESTSIG;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_fork() -> Result<(), Error> {
        let test_exit_code = 42;

        let cg = cgroups::new("test_fork".to_string())?;

        // Use pipes to signal from the child to the parent
        let (r, w) = pipe2(OFlag::O_CLOEXEC).unwrap();

        let ret = unsafe { fork(Some(cg.as_ref())) };

        match ret {
            Ok(Context::Parent(tid, pidfd)) => {
                // Make sure the child has setup signal handlers
                let res = read(r, &mut vec![0]);
                _ = close(r);
                res.unwrap();

                // check that the pid is running
                assert!(pidfd.is_running()?);

                pidfd.kill(TESTSIG)?;

                // Wait for the child to exit before trying to delete the cgroup
                let status = pidfd
                    .wait()
                    .map_err(|e| Error::Others(format!("error waiting for pidfd: {}", e)));

                assert!(!pidfd.is_running()?);

                if has_cap_sys_admin() {
                    cg.delete()
                        .map_err(|e| Error::Others(format!("error deleting cgroup: {}", e)))?;
                }
                assert!(tid > 0);
                let status = status?;
                if status.status != test_exit_code {
                    return Err(Error::Others(format!(
                        "unexpected exit status, expected {}: {:?}",
                        test_exit_code, status
                    )));
                }
                return Ok(());
            }
            Ok(Context::Child) => {
                let term = Arc::new(AtomicBool::new(false));
                signal_hook::flag::register(TESTSIG, Arc::clone(&term))?;

                _ = write(w, b"1");
                _ = close(w);

                while !term.load(Ordering::Relaxed) {}

                std::process::exit(test_exit_code.try_into().unwrap());
            }
            Err(e) => {
                return Err(e);
            }
        }
    }

    #[test]
    fn test_fork_sudo() -> Result<(), Error> {
        if has_cap_sys_admin() {
            return test_fork();
        }
        run_test_with_sudo(function!())
    }
}
