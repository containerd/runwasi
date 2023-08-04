use containerd_shim_wasm::sandbox::oci;
use libcontainer::workload::{Executor, ExecutorError, EMPTY};
use nix::unistd::{dup, dup2};

use std::ffi::CString;
use std::io::Read;
use std::{fs::OpenOptions, os::fd::RawFd, path::PathBuf};

use libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use nix::unistd;
use oci_spec::runtime::Spec;

const EXECUTOR_NAME: &str = "default";

#[derive(Default)]
pub struct LinuxContainerExecutor {
    pub stdin: Option<RawFd>,
    pub stdout: Option<RawFd>,
    pub stderr: Option<RawFd>,
}

impl Executor for LinuxContainerExecutor {
    fn exec(&self, spec: &Spec) -> Result<(), ExecutorError> {
        log::debug!("executing workload with default handler");
        let args = spec
            .process()
            .as_ref()
            .and_then(|p| p.args().as_ref())
            .unwrap_or(&EMPTY);

        if args.is_empty() {
            log::error!("no arguments provided to execute");
            Err(ExecutorError::InvalidArg)?;
        }

        redirect_io(self.stdin, self.stdout, self.stderr).map_err(|err| {
            log::error!("failed to redirect io: {}", err);
            ExecutorError::Other(format!("failed to redirect io: {}", err))
        })?;

        let executable = args[0].as_str();
        let cstring_path = CString::new(executable.as_bytes()).map_err(|err| {
            log::error!("failed to convert path {executable:?} to cstring: {}", err,);
            ExecutorError::InvalidArg
        })?;
        let a: Vec<CString> = args
            .iter()
            .map(|s| CString::new(s.as_bytes()).unwrap_or_default())
            .collect();
        unistd::execvp(&cstring_path, &a).map_err(|err| {
            log::error!("failed to execvp: {}", err);
            ExecutorError::Execution(err.into())
        })?;

        // After execvp is called, the process is replaced with the container
        // payload through execvp, so it should never reach here.
        unreachable!();
    }

    fn can_handle(&self, spec: &Spec) -> bool {
        let args = oci::get_args(spec);

        if args.is_empty() {
            return false;
        }

        let executable = args[0].as_str();

        // mostly follows youki's verify_binary implementation
        // https://github.com/containers/youki/blob/2d6fd7650bb0f22a78fb5fa982b5628f61fe25af/crates/libcontainer/src/process/container_init_process.rs#L106
        let path = if executable.contains('/') {
            PathBuf::from(executable)
        } else {
            let path = std::env::var("PATH").unwrap_or_default();
            // check each path in $PATH
            let mut found = false;
            let mut found_path = PathBuf::default();
            for p in path.split(':') {
                let path = PathBuf::from(p).join(executable);
                if path.exists() {
                    found = true;
                    found_path = path;
                    break;
                }
            }
            if !found {
                return false;
            }
            found_path
        };

        // check execute permission
        use std::os::unix::fs::PermissionsExt;
        let metadata = path.metadata();
        if metadata.is_err() {
            log::info!("failed to get metadata of {:?}", path);
            return false;
        }
        let metadata = metadata.unwrap();
        let permissions = metadata.permissions();
        if !metadata.is_file() || permissions.mode() & 0o001 == 0 {
            log::info!("{} is not a file or has no execute permission", executable);
            return false;
        }

        // check the shebang and ELF magic number
        // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format#File_header
        let mut buffer = [0; 4];

        let file = OpenOptions::new().read(true).open(path);
        if file.is_err() {
            log::info!("failed to open {}", executable);
            return false;
        }
        let mut file = file.unwrap();
        match file.read_exact(&mut buffer) {
            Ok(_) => {}
            Err(err) => {
                log::info!("failed to read shebang of {}: {}", executable, err);
                return false;
            }
        }
        match buffer {
            // ELF magic number
            [0x7f, 0x45, 0x4c, 0x46] => true,
            // shebang
            [0x23, 0x21, ..] => true,
            _ => {
                log::info!("{} is not a valid script or elf file", executable);
                false
            }
        }
    }

    fn name(&self) -> &'static str {
        EXECUTOR_NAME
    }
}

fn redirect_io(stdin: Option<i32>, stdout: Option<i32>, stderr: Option<i32>) -> anyhow::Result<()> {
    if let Some(stdin) = stdin {
        dup(STDIN_FILENO)?;
        dup2(stdin, STDIN_FILENO)?;
    }
    if let Some(stdout) = stdout {
        dup(STDOUT_FILENO)?;
        dup2(stdout, STDOUT_FILENO)?;
    }
    if let Some(stderr) = stderr {
        dup(STDERR_FILENO)?;
        dup2(stderr, STDERR_FILENO)?;
    }
    Ok(())
}
