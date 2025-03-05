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

/// Macros for logging with context information (container ID) automatically included.
#[macro_export]
macro_rules! log_with_ctx {
    // Info level log
    (info, $ctx:expr, $($arg:tt)+) => {
        {
            let ctx = $ctx;
            match ctx.pod_id() {
                Some(pod_id) => log::info!(instance = ctx.container_id(), pod = pod_id; $($arg)+),
                None => log::info!(instance = ctx.container_id(); $($arg)+)
            }
        }
    };

    // Debug level log
    (debug, $ctx:expr, $($arg:tt)+) => {
        {
            let ctx = $ctx;
            match ctx.pod_id() {
                Some(pod_id) => log::debug!(instance = ctx.container_id(), pod = pod_id; $($arg)+),
                None => log::debug!(instance = ctx.container_id(); $($arg)+)
            }
        }
    };

    // Warn level log
    (warn, $ctx:expr, $($arg:tt)+) => {
        {
            let ctx = $ctx;
            match ctx.pod_id() {
                Some(pod_id) => log::warn!(instance = ctx.container_id(), pod = pod_id; $($arg)+),
                None => log::warn!(instance = ctx.container_id(); $($arg)+)
            }
        }
    };

    // Error level log
    (error, $ctx:expr, $($arg:tt)+) => {
        {
            let ctx = $ctx;
            match ctx.pod_id() {
                Some(pod_id) => log::error!(instance = ctx.container_id(), pod = pod_id; $($arg)+),
                None => log::error!(instance = ctx.container_id(); $($arg)+)
            }
        }
    };

    // Trace level log
    (trace, $ctx:expr, $($arg:tt)+) => {
        {
            let ctx = $ctx;
            match ctx.pod_id() {
                Some(pod_id) => log::trace!(instance = ctx.container_id(), pod = pod_id; $($arg)+),
                None => log::trace!(instance = ctx.container_id(); $($arg)+)
            }
        }
    };
}

/// Convenience macro for info level logs
#[macro_export]
macro_rules! info {
    ($ctx:expr, $($arg:tt)+) => {
        $crate::log_with_ctx!(info, $ctx, $($arg)+)
    };
}

/// Convenience macro for debug level logs
#[macro_export]
macro_rules! debug {
    ($ctx:expr, $($arg:tt)+) => {
        $crate::log_with_ctx!(debug, $ctx, $($arg)+)
    };
}

/// Convenience macro for warn level logs
#[macro_export]
macro_rules! warn {
    ($ctx:expr, $($arg:tt)+) => {
        $crate::log_with_ctx!(warn, $ctx, $($arg)+)
    };
}

/// Convenience macro for error level logs
#[macro_export]
macro_rules! error {
    ($ctx:expr, $($arg:tt)+) => {
        $crate::log_with_ctx!(error, $ctx, $($arg)+)
    };
}

/// Convenience macro for trace level logs
#[macro_export]
macro_rules! trace {
    ($ctx:expr, $($arg:tt)+) => {
        $crate::log_with_ctx!(trace, $ctx, $($arg)+)
    };
}
