//! # Logging Utilities
//!
//! This module provides structured logging macros that automatically include container runtime  
//! context information in log messages, making it easier to trace and debug container operations.
//!
//! ## Usage
//!
//! To use these macros, you need to have a context object that implements methods:
//! - `container_id()` - returns the ID of the container
//! - `pod_id()` - returns an Option containing the pod ID if available
//!
//! ### Example
//!
//! ```ignore
//! containerd_shim_wasm::info!(ctx, "Starting container initialization");
//! ```
//!
//! The resulting log entries will automatically include the container ID and pod ID (if available)
//! as structured fields, making it easier to filter and analyze logs.
//!

macro_rules! make_log {
    ($d:tt, $level:ident) => {
        /// Convenience macro for $level level logs
        #[macro_export]
        macro_rules! $level {
            ($d ctx:expr, $d($d arg:tt)+) => {
                {
                    let ctx = $d ctx;
                    match ctx.pod_id() {
                        Some(pod_id) => log::$level!(instance = ctx.container_id(), pod = pod_id; $d($d arg)+),
                        None => log::$level!(instance = ctx.container_id(); $d($d arg)+)
                    }
                }
            };
        }
    };
    ($d:tt, $level:ident, $($rest:ident),+) => {
        make_log! { $d, $level }
        make_log! { $d, $($rest),+ }
    };
}

make_log! {$, info, debug, warn, error, trace }
