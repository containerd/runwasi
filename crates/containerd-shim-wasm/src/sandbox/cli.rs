use std::path::PathBuf;

use containerd_shim::{parse, run, Config};

#[cfg(feature = "opentelemetry")]
use crate::sandbox::shim::{otel_traces_enabled, OtlpConfig};
use crate::sandbox::{Instance, ShimCli};

pub mod r#impl {
    pub use git_version::git_version;
}

pub use crate::{revision, version};

#[macro_export]
macro_rules! version {
    () => {
        env!("CARGO_PKG_VERSION")
    };
}

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

/// Main entry point for the shim.
///
/// If the `opentelemetry` feature is enabled, this function will start the shim with OpenTelemetry tracing.
///
/// It parses OTLP configuration from the environment and initializes the OpenTelemetry SDK.
pub fn shim_main<'a, I>(
    name: &str,
    version: &str,
    revision: impl Into<Option<&'a str>>,
    shim_version: impl Into<Option<&'a str>>,
    config: Option<Config>,
) where
    I: 'static + Instance + Sync + Send,
    I::Engine: Default,
{
    #[cfg(feature = "opentelemetry")]
    if otel_traces_enabled() {
        // opentelemetry uses tokio, so we need to initialize a runtime
        use tokio::runtime::Runtime;
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let _guard = OtlpConfig::build_from_env()
                .expect("Failed to build OtelConfig.")
                .init()
                .expect("Failed to initialize OpenTelemetry.");
            shim_main_inner::<I>(name, version, revision, shim_version, config);
        });
    } else {
        shim_main_inner::<I>(name, version, revision, shim_version, config);
    }

    #[cfg(not(feature = "opentelemetry"))]
    {
        shim_main_inner::<I>(name, version, revision, shim_version, config);
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
fn shim_main_inner<'a, I>(
    name: &str,
    version: &str,
    revision: impl Into<Option<&'a str>>,
    shim_version: impl Into<Option<&'a str>>,
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
            std::env::set_var("TRACECONTEXT", ctx);
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

    run::<ShimCli<I>>(&shim_id, config);
}
