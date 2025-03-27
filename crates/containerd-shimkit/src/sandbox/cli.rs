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
//! ```rust,no_run
//! use containerd_shimkit::Config;
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
//! - [`version!()`] - Returns the crate version from Cargo.toml
//! - [`revision!()`] - Returns the Git revision hash, if available
//!
//! ## Example usage:
//!
//! ```rust,no_run
//! use containerd_shimkit::{
//!     shim_version,
//!     sandbox::{Instance, InstanceConfig, Result},
//!     sandbox::cli::shim_main,
//!     sandbox::sync::WaitableCell,
//!     Config,
//! };
//! use tokio::time::sleep;
//! use chrono::{DateTime, Utc};
//! use std::time::Duration;
//!
//! #[derive(Clone, Default)]
//! struct MyInstance {
//!     exit_code: WaitableCell<(u32, DateTime<Utc>)>,
//! };
//!
//! impl Instance for MyInstance {
//!     async fn new(id: String, cfg: &InstanceConfig) -> Result<Self> {
//!         let exit_code = WaitableCell::new();
//!         Ok(Self { exit_code })
//!     }
//!     async fn start(&self) -> Result<u32> {
//!         let exit_code = self.exit_code.clone();
//!         tokio::spawn(async move {
//!             sleep(Duration::from_millis(100)).await;
//!             let _ = exit_code.set((0, Utc::now()));
//!         });
//!         Ok(42) // some id for our task, usually a PID
//!     }
//!     async fn kill(&self, signal: u32) -> Result<()> {
//!         Ok(()) // no-op
//!     }
//!     async fn delete(&self) -> Result<()> {
//!         Ok(()) // no-op
//!     }
//!     async fn wait(&self) -> (u32, DateTime<Utc>) {
//!         *self.exit_code.wait().await
//!     }
//! }
//!
//! let config = Config {
//!     default_log_level: "error".to_string(),
//!     ..Default::default()
//! };
//!
//! shim_main::<MyInstance>(
//!     "my-engine",
//!     shim_version!(),
//!     Some(config),
//! );
//! ```
//!
//! When the `opentelemetry` feature is enabled, additional runtime config
//! is available through environment variables:
//!
//! - `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`: Enable OpenTelemetry tracing
//! - `OTEL_EXPORTER_OTLP_ENDPOINT`: Enable OpenTelemetry tracing as above
//! - `OTEL_SDK_DISABLED`: Disable OpenTelemetry SDK
//!

use std::path::PathBuf;

use containerd_shim::{Config, parse, run};

#[cfg(feature = "opentelemetry")]
use crate::sandbox::async_utils::AmbientRuntime as _;
#[cfg(feature = "opentelemetry")]
use crate::sandbox::shim::{OtlpConfig, otel_traces_enabled};
use crate::sandbox::{Instance, Shim};

pub mod r#impl {
    pub use git_version::git_version;
}

pub use crate::{revision, version};

/// Get the crate version from Cargo.toml.
#[macro_export]
macro_rules! version {
    () => {
        option_env!("CARGO_PKG_VERSION")
    };
}

/// Get the Git revision hash, if available.
#[macro_export]
macro_rules! revision {
    () => {
        match $crate::sandbox::cli::r#impl::git_version!(
            args = ["--match=:", "--always", "--abbrev=15", "--dirty=.m"],
            fallback = "",
        ) {
            "" => None,
            version => Some(version),
        }
    };
}

pub struct Version {
    pub version: &'static str,
    pub revision: &'static str,
}

#[macro_export]
macro_rules! shim_version {
    () => {
        $crate::sandbox::cli::Version {
            version: $crate::version!().unwrap_or("<none>"),
            revision: $crate::revision!().unwrap_or("<none>"),
        }
    };
}

impl Default for Version {
    fn default() -> Self {
        Self {
            version: "<none>",
            revision: "<none>",
        }
    }
}

#[cfg(target_os = "linux")]
fn get_mem(pid: u32) -> (usize, usize) {
    let mut rss = 0;
    let mut total = 0;
    for line in std::fs::read_to_string(format!("/proc/{pid}/status"))
        .unwrap()
        .lines()
    {
        let line = line.trim();
        // VmPeak is the maximum total virtual memory used so far.
        // VmHWM (high water mark) is the maximum resident set memory used so far.
        // See: https://man7.org/linux/man-pages/man5/proc_pid_status.5.html
        if let Some(rest) = line.strip_prefix("VmPeak:") {
            if let Some(rest) = rest.strip_suffix("kB") {
                total = rest.trim().parse().unwrap_or(0);
            }
        } else if let Some(rest) = line.strip_prefix("VmHWM:") {
            if let Some(rest) = rest.strip_suffix("kB") {
                rss = rest.trim().parse().unwrap_or(0);
            }
        }
    }
    (rss, total)
}

#[cfg(target_os = "linux")]
fn log_mem() {
    let pid = std::process::id();
    let (rss, tot) = get_mem(pid);
    log::info!("Shim peak memory usage was: peak resident set {rss} kB, peak total {tot} kB");

    let pid = zygote::Zygote::global().run(|_| std::process::id(), ());
    let (rss, tot) = get_mem(pid);
    log::info!("Zygote peak memory usage was: peak resident set {rss} kB, peak total {tot} kB");
}

#[cfg(unix)]
fn init_zygote_and_logger(debug: bool, config: &Config) {
    zygote::Zygote::init();
    if config.no_setup_logger {
        return;
    }
    zygote::Zygote::global().run(
        |(debug, default_log_level)| {
            // last two arguments are unused in unix
            crate::vendor::containerd_shim::logger::init(debug, &default_log_level, "", "")
                .expect("Failed to initialize logger");
        },
        (debug, config.default_log_level.clone()),
    );
}

/// Main entry point for the shim.
///
/// If the `opentelemetry` feature is enabled, this function will start the shim with OpenTelemetry tracing.
///
/// It parses OTLP configuration from the environment and initializes the OpenTelemetry SDK.
pub fn shim_main<I>(name: &str, version: Version, config: Option<Config>)
where
    I: 'static + Instance + Sync + Send,
{
    // parse the version flag
    let os_args: Vec<_> = std::env::args_os().collect();

    let flags = parse(&os_args[1..]).unwrap();
    let argv0 = PathBuf::from(&os_args[0]);
    let argv0 = argv0.file_stem().unwrap_or_default().to_string_lossy();

    if flags.version {
        println!("{argv0}:");
        println!("  Runtime: {name}");
        println!("  Version: {}", version.version);
        println!("  Revision: {}", version.revision);
        println!();

        std::process::exit(0);
    }

    // Initialize the zygote and logger for the container process
    #[cfg(unix)]
    {
        let default_config = Config::default();
        let config = config.as_ref().unwrap_or(&default_config);
        init_zygote_and_logger(flags.debug, config);
    }

    #[cfg(feature = "opentelemetry")]
    if otel_traces_enabled() {
        // opentelemetry uses tokio, so we need to initialize a runtime
        async {
            let otlp_config = OtlpConfig::build_from_env().expect("Failed to build OtelConfig.");
            let _guard = otlp_config
                .init()
                .expect("Failed to initialize OpenTelemetry.");
            tokio::task::block_in_place(move || {
                shim_main_inner::<I>(name, config);
            });
        }
        .block_on();
    } else {
        shim_main_inner::<I>(name, config);
    }

    #[cfg(not(feature = "opentelemetry"))]
    {
        shim_main_inner::<I>(name, config);
    }

    #[cfg(target_os = "linux")]
    log_mem();
}

#[cfg_attr(feature = "tracing", tracing::instrument(level = "Info"))]
fn shim_main_inner<I>(name: &str, config: Option<Config>)
where
    I: 'static + Instance + Sync + Send,
{
    #[cfg(feature = "opentelemetry")]
    {
        // read TRACECONTEXT env var that's set by the parent process
        if let Ok(ctx) = std::env::var("TRACECONTEXT") {
            OtlpConfig::set_trace_context(&ctx).unwrap();
        } else {
            let ctx = OtlpConfig::get_trace_context().unwrap();
            // SAFETY: although it's in a multithreaded context,
            // it's safe to assume that all the other threads are not
            // messing with env vars at this point.
            unsafe {
                std::env::set_var("TRACECONTEXT", ctx);
            }
        }
    }

    run::<Shim<I>>(name, config);
}
