//! Command line interface for the containerd shim.
//!
//! The CLI provides the interface between containerd and the Wasm runtime.
//! It handles commands like start and delete from containerd's shim API.
//!
//! ## Usage
//!
//! The shim binary should be named `containerd-shim-<engine>-v1` and installed in $PATH.
//! containerd will call the shim with various commands.
//!
//! ## Configuration
//!
//! The shim can be configured using the [`Config`] struct:
//!
//! ```rust, no_run
//! use containerd_shim_wasm::Config;
//!
//! let config = Config {
//!     // Disable automatic logger setup
//!     no_setup_logger: false,
//!     // Set default log level
//!     default_log_level: "info".to_string(),
//!     // Disable child process reaping
//!     no_reaper: false,
//!     // Disable subreaper setting
//!     no_sub_reaper: false,
//! };
//! ```
//!
//! ## Version Information
//!
//! The module provides two macros for version information:
//!
//! - [`version!()`](crate::version) - Returns the crate version from Cargo.toml and
//!   Git revision hash, if available.
//!
//! ## Example usage:
//!
//! ```rust, no_run
//! use containerd_shim_wasm::{
//!     Cli, version,
//!     shim::{Shim, Sandbox, RuntimeContext, Version},
//!     Config,
//! };
//! use anyhow::Result;
//!
//! struct MyShim;
//!
//! #[derive(Default)]
//! struct MySandbox;
//!
//! impl Shim for MyShim {
//!     type Sandbox = MySandbox;
//!
//!     fn name() -> &'static str {
//!         "my-shim"
//!     }
//!
//!     fn version() -> Version {
//!         version!()
//!     }
//! }
//!
//! impl Sandbox for MySandbox {
//!     async fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32> {
//!         Ok(0)
//!     }
//! }
//!
//! let config = Config {
//!     default_log_level: "error".to_string(),
//!     ..Default::default()
//! };
//!
//! MyShim::run(config);
//! ```
//!
//! When the `opentelemetry` feature is enabled, additional runtime config
//! is available through environment variables:
//!
//! - `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`: Enable OpenTelemetry tracing
//! - `OTEL_EXPORTER_OTLP_ENDPOINT`: Enable OpenTelemetry tracing as above
//! - `OTEL_SDK_DISABLED`: Disable OpenTelemetry SDK
//!

use crate::Config;
use crate::shim::{Instance, Shim};

mod private {
    pub trait Sealed {}
}

impl<S: Shim> private::Sealed for S {}

pub trait Cli: Shim + private::Sealed {
    /// Main entry point for the shim.
    ///
    /// If the `opentelemetry` feature is enabled, this function will start the shim with OpenTelemetry tracing.
    ///
    /// It parses OTLP configuration from the environment and initializes the OpenTelemetry SDK.
    fn run(config: impl Into<Option<Config>>);
}

impl<S: Shim> Cli for S {
    fn run(config: impl Into<Option<Config>>) {
        containerd_shimkit::sandbox::cli::shim_main::<Instance<S>>(
            S::name(),
            S::version(),
            config.into(),
        )
    }
}
