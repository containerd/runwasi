//! The shim exposes the [Config] struct to configure the shim and [OtlpConfig] module to enable tracing if the `opentelemetry` feature is enabled.

pub use local::Config;

mod events;
mod instance_data;
mod local;
#[allow(clippy::module_inception)]
mod shim;
mod task_state;
pub(crate) use shim::Shim;

#[cfg(feature = "opentelemetry")]
mod otel;
#[cfg(feature = "opentelemetry")]
pub(crate) use otel::{Config as OtlpConfig, traces_enabled as otel_traces_enabled};
