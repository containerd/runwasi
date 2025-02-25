/// Macros for logging with context information (container ID) automatically included.
#[macro_export]
macro_rules! log_with_ctx {
    // Info level log
    (info, $ctx:expr, $($arg:tt)+) => {
        log::info!(instance = $ctx.container_id(); $($arg)+)
    };

    // Debug level log
    (debug, $ctx:expr, $($arg:tt)+) => {
        log::debug!(instance = $ctx.container_id(); $($arg)+)
    };

    // Warn level log
    (warn, $ctx:expr, $($arg:tt)+) => {
        log::warn!(instance = $ctx.container_id(); $($arg)+)
    };

    // Error level log
    (error, $ctx:expr, $($arg:tt)+) => {
        log::error!(instance = $ctx.container_id(); $($arg)+)
    };

    // Trace level log
    (trace, $ctx:expr, $($arg:tt)+) => {
        log::trace!(instance = $ctx.container_id(); $($arg)+)
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
