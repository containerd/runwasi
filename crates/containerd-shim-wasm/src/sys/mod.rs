#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub mod metrics {
    #[cfg(unix)]
    pub use crate::sys::unix::get_metrics;
    #[cfg(windows)]
    pub use crate::sys::windows::get_metrics;
}

pub mod networking {
    #[cfg(unix)]
    pub use crate::sys::unix::setup_namespaces;
    #[cfg(windows)]
    pub use crate::sys::windows::setup_namespaces;
}
