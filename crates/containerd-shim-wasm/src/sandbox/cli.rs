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
//! - [`version!()`] - Returns the crate version from Cargo.toml
//! - [`revision!()`] - Returns the Git revision hash, if available
//!
//! ## Example usage:
//!
//! ```rust, no_run
//! use containerd_shim_wasm::{
//!     container::{Instance, Engine, RuntimeContext},
//!     sandbox::cli::{revision, shim_main, version},
//!     Config,
//! };
//! use anyhow::Result;
//!
//! #[derive(Clone, Default)]
//! struct MyEngine;
//!
//! impl Engine for MyEngine {
//!     fn name() -> &'static str {
//!         "my-engine"
//!     }
//!
//!     fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32> {
//!         Ok(0)
//!     }
//! }
//!
//! let config = Config {
//!     default_log_level: "error".to_string(),
//!     ..Default::default()
//! };
//!
//! shim_main::<Instance<MyEngine>>(
//!     "my-engine",
//!     version!(),
//!     revision!(),
//!     "v1",
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
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, LazyLock};

use containerd_shim::{Config, parse, run};

#[cfg(feature = "opentelemetry")]
use crate::sandbox::shim::{OtlpConfig, otel_traces_enabled};
use crate::sandbox::{Instance, ShimCli};

pub mod r#impl {
    pub use git_version::git_version;
}

use super::async_utils::AmbientRuntime as _;
pub use crate::{revision, version};

/// Get the crate version from Cargo.toml.
#[macro_export]
macro_rules! version {
    () => {
        env!("CARGO_PKG_VERSION")
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

#[cfg(target_os = "linux")]
fn get_stats(pid: u32) -> (usize, usize, usize) {
    let mut rss = 0;
    let mut total = 0;
    let mut threads = 0;
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
        } else if let Some(rest) = line.strip_prefix("Threads:") {
            threads = rest.trim().parse().unwrap_or(0);
        }
    }
    (rss, total, threads)
}

#[cfg(target_os = "linux")]
fn monitor_treads() -> usize {
    use std::sync::atomic::Ordering::SeqCst;
    use std::time::Duration;

    use tokio::time::sleep;

    static NUM_THREADS: LazyLock<Arc<AtomicUsize>> = LazyLock::new(|| {
        let pid = std::process::id();
        let num_threads = Arc::new(AtomicUsize::new(0));
        let n = num_threads.clone();
        async move {
            loop {
                let (_, _, threads) = get_stats(pid);
                n.fetch_max(threads, SeqCst);
                sleep(Duration::from_millis(10)).await;
            }
        }
        .spawn();
        num_threads
    });
    NUM_THREADS.load(SeqCst)
}

#[cfg(target_os = "linux")]
fn log_stats() {
    let pid = std::process::id();
    let (rss, tot, _) = get_stats(pid);
    log::info!("Shim peak memory usage was: peak resident set {rss} kB, peak total {tot} kB");

    let threads = monitor_treads();
    log::info!("Shim peak number of threads was {threads}");

    let pid = zygote::Zygote::global().run(|_| std::process::id(), ());
    let (rss, tot, _) = get_stats(pid);
    log::info!("Zygote peak memory usage was: peak resident set {rss} kB, peak total {tot} kB");
}

/// Main entry point for the shim.
///
/// If the `opentelemetry` feature is enabled, this function will start the shim with OpenTelemetry tracing.
///
/// It parses OTLP configuration from the environment and initializes the OpenTelemetry SDK.
pub fn shim_main<'a, I>(
    name: &str,
    version: &str,
    revision: impl Into<Option<&'a str>> + std::fmt::Debug,
    shim_version: impl Into<Option<&'a str>> + std::fmt::Debug,
    config: Option<Config>,
) where
    I: 'static + Instance + Sync + Send,
    I::Engine: Default,
{
    #[cfg(unix)]
    zygote::Zygote::init();

    #[cfg(unix)]
    monitor_treads();

    async {
        #[cfg(feature = "opentelemetry")]
        if otel_traces_enabled() {
            // opentelemetry uses tokio, so we need to initialize a runtime
            let otlp_config = OtlpConfig::build_from_env().expect("Failed to build OtelConfig.");
            let _guard = otlp_config
                .init()
                .expect("Failed to initialize OpenTelemetry.");
            shim_main_inner::<I>(name, version, revision, shim_version, config).await;
        } else {
            shim_main_inner::<I>(name, version, revision, shim_version, config).await;
        };

        #[cfg(not(feature = "opentelemetry"))]
        shim_main_inner::<I>(name, version, revision, shim_version, config).await;
    }
    .block_on();

    #[cfg(target_os = "linux")]
    log_stats();
}

#[cfg_attr(feature = "tracing", tracing::instrument(level = "Info"))]
async fn shim_main_inner<'a, I>(
    name: &str,
    version: &str,
    revision: impl Into<Option<&'a str>> + std::fmt::Debug,
    shim_version: impl Into<Option<&'a str>> + std::fmt::Debug,
    config: Option<Config>,
) where
    I: 'static + Instance + Sync + Send,
    I::Engine: Default,
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
    let os_args: Vec<_> = std::env::args_os().collect();

    let flags = parse(&os_args[1..]).unwrap();
    let argv0 = PathBuf::from(&os_args[0]);
    let argv0 = argv0.file_stem().unwrap_or_default().to_string_lossy();

    if flags.version {
        println!("{argv0}:");
        println!("  Runtime: {name}");
        println!("  Version: {version}");
        println!("  Revision: {}", revision.into().unwrap_or("<none>"));
        println!();

        std::process::exit(0);
    }

    let shim_version = shim_version.into().unwrap_or("v1");

    let lower_name = name.to_lowercase();
    let shim_id = format!("io.containerd.{lower_name}.{shim_version}");

    run::<ShimCli<I>>(&shim_id, config).await;
}
