use std::fs::OpenOptions;
use std::io::Read;
use std::os::fd::RawFd;
use std::path::PathBuf;

use libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use libcontainer::workload::default::DefaultExecutor;
use libcontainer::workload::{Executor, ExecutorError};
use nix::unistd::{dup, dup2};
use oci_spec::runtime::Spec;

use crate::sandbox::oci;

#[derive(Default)]
pub struct LinuxContainerExecutor {
    stdin: Option<RawFd>,
    stdout: Option<RawFd>,
    stderr: Option<RawFd>,
    default_executor: DefaultExecutor,
}

impl LinuxContainerExecutor {
    pub fn new(stdin: Option<RawFd>, stdout: Option<RawFd>, stderr: Option<RawFd>) -> Self {
        Self {
            stdin,
            stdout,
            stderr,
            ..Default::default()
        }
    }
}

impl Executor for LinuxContainerExecutor {
    fn exec(&self, spec: &Spec) -> Result<(), ExecutorError> {
        redirect_io(self.stdin, self.stdout, self.stderr).map_err(|err| {
            log::error!("failed to redirect io: {}", err);
            ExecutorError::Other(format!("failed to redirect io: {}", err))
        })?;
        self.default_executor.exec(spec)
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
        self.default_executor.name()
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
