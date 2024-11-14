//! The shim is the entrypoint for the containerd shim API. It is responsible
//! for commmuincating with the containerd daemon and managing the lifecycle of
//! the container/sandbox.

mod cli;
mod events;
mod instance_data;
mod local;
#[cfg(feature = "opentelemetry")]
mod otel;
mod task_state;

pub use cli::Cli;
#[cfg(feature = "opentelemetry")]
pub use otel::{traces_enabled as otel_traces_enabled, Config as OtlpConfig};
