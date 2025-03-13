use std::path::PathBuf;
use std::sync::LazyLock;

pub mod metrics;
pub mod stdio;

pub static DEFAULT_CONTAINER_ROOT_DIR: LazyLock<PathBuf> = LazyLock::new(|| {
    let program_data = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    program_data.join("containerd").join("state")
});
