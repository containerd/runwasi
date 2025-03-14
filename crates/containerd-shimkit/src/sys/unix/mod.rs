use std::path::PathBuf;
use std::sync::LazyLock;

pub mod metrics;
pub mod stdio;

pub static DEFAULT_CONTAINER_ROOT_DIR: LazyLock<PathBuf> =
    LazyLock::new(|| PathBuf::from("/run/containerd"));
