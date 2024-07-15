//! OpenTelemetry Configuration Module
//!
//! This module provides a configuration structure and associated methods to initialize
//! OpenTelemetry tracing with the OTLP exporter. The configuration can be set up via
//! the `OtelConfig` struct and its builder pattern.
//!
//! # Usage
//!
//! ```rust
//! use containerd-shim-wasm::sandbox::shim::otel::OtelConfig;
//!
//! fn main() -> anyhow::Result<()> {
//!     let otel_config = OtelConfig::builder()
//!         .otel_endpoint_from_env()
//!         .build()?;
//!
//!     let _guard = otel_config.init()?;
//!
//!     // Your application code here
//!
//!     Ok(())
//! }
//! ```

use std::collections::HashMap;
use std::env;

use opentelemetry::global::{self, set_text_map_propagator};
use opentelemetry::trace::TraceError;
use opentelemetry_otlp::{
    SpanExporterBuilder, WithExportConfig, OTEL_EXPORTER_OTLP_PROTOCOL_DEFAULT,
};
pub use opentelemetry_otlp::{
    OTEL_EXPORTER_OTLP_ENDPOINT, OTEL_EXPORTER_OTLP_PROTOCOL, OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::{runtime, trace as sdktrace};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::{EnvFilter, Registry};

const OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF: &str = "http/protobuf";
const OTEL_EXPORTER_OTLP_PROTOCOL_GRPC: &str = "grpc";
const OTEL_EXPORTER_OTLP_TRACES_PROTOCOL: &str = "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL";

/// Configuration struct for OpenTelemetry setup.
pub struct Config {
    otel_endpoint: String,
    otel_protocol: String,
}

/// Initializes a new OpenTelemetry tracer with the OTLP exporter.
///
/// Returns a `Result` containing the initialized tracer or a `TraceError` if initialization fails.
///
/// https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/protocol/exporter.md#configuration-options
impl Config {
    /// Creates a new builder for `OtelConfig`.
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::default()
    }

    /// Returns `true` if traces are enabled, `false` otherwise.
    ///
    /// Traces are enabled if either `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` or `OTEL_EXPORTER_OTLP_ENDPOINT` is set and not empty.
    pub fn traces_enabled() -> bool {
        let traces_endpoint = env::var(OTEL_EXPORTER_OTLP_TRACES_ENDPOINT).ok();
        let otlp_endpoint = env::var(OTEL_EXPORTER_OTLP_ENDPOINT).ok();

        traces_endpoint.map_or(false, |v| !v.is_empty())
            || otlp_endpoint.map_or(false, |v| !v.is_empty())
    }

    /// Initializes the tracer, sets up the telemetry and subscriber layers, and sets the global subscriber.
    ///
    /// Note: this function should be called only once and be called by the binary entry point.
    pub fn init(&self) -> anyhow::Result<ShutdownGuard> {
        let tracer = self.init_tracer()?;
        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
        set_text_map_propagator(TraceContextPropagator::new());

        let filter = EnvFilter::try_new("info,h2=off")?;

        let subscriber = Registry::default().with(telemetry).with(filter);

        tracing::subscriber::set_global_default(subscriber)?;
        Ok(ShutdownGuard)
    }

    /// Returns the current trace context as a JSON string.
    pub fn get_trace_context() -> anyhow::Result<String> {
        // propogate the context
        let mut injector: HashMap<String, String> = HashMap::new();
        global::get_text_map_propagator(|propagator| {
            // retrieve the context from `tracing`
            propagator.inject_context(&Span::current().context(), &mut injector);
        });
        Ok(serde_json::to_string(&injector)?)
    }

    /// Sets the trace context from a JSON string.
    pub fn set_trace_context(trace_context: &str) -> anyhow::Result<()> {
        let extractor: HashMap<String, String> = serde_json::from_str(trace_context)?;
        let context = global::get_text_map_propagator(|propagator| propagator.extract(&extractor));
        Span::current().set_parent(context);
        Ok(())
    }

    fn init_tracer_http_protobuf(&self) -> SpanExporterBuilder {
        opentelemetry_otlp::new_exporter()
            .http()
            .with_endpoint(&self.otel_endpoint)
            .into()
    }

    fn init_tracer_grpc(&self) -> SpanExporterBuilder {
        opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint(&self.otel_endpoint)
            .into()
    }

    fn init_tracer(&self) -> Result<opentelemetry_sdk::trace::Tracer, TraceError> {
        let exporter = match self.otel_protocol.as_str() {
            OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF => self.init_tracer_http_protobuf(),
            OTEL_EXPORTER_OTLP_PROTOCOL_GRPC => self.init_tracer_grpc(),
            _ => Err(TraceError::from(
                "Invalid OTEL_EXPORTER_OTLP_PROTOCOL value",
            ))?,
        };

        opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(exporter)
            .with_trace_config(sdktrace::config())
            .install_batch(runtime::Tokio)
    }
}

/// Shutdown of the open telemetry services will automatically called when the OtelConfig instance goes out of scope.
#[must_use]
pub struct ShutdownGuard;

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        // Give tracer provider a chance to flush any pending traces.
        opentelemetry::global::shutdown_tracer_provider();
    }
}

#[derive(Default)]
pub struct ConfigBuilder {
    otel_endpoint: Option<String>,
    otel_protocol: String,
}

impl ConfigBuilder {
    /// Sets the OTLP endpoint from environment variables.
    pub fn otel_endpoint_from_env(mut self) -> Self {
        self.otel_endpoint = env::var(OTEL_EXPORTER_OTLP_TRACES_ENDPOINT)
            .or_else(|_| env::var(OTEL_EXPORTER_OTLP_ENDPOINT))
            .ok();
        self
    }

    /// Sets the OTLP protocol from environment variables.
    pub fn otel_protocol_from_env(mut self) -> Self {
        self.otel_protocol = env::var(OTEL_EXPORTER_OTLP_TRACES_PROTOCOL).unwrap_or(
            env::var(OTEL_EXPORTER_OTLP_PROTOCOL)
                .unwrap_or(OTEL_EXPORTER_OTLP_PROTOCOL_DEFAULT.to_owned()),
        );
        self
    }

    /// Builds the `OtelConfig` instance.
    pub fn build(self) -> Result<Config, &'static str> {
        Ok(Config {
            otel_endpoint: self.otel_endpoint.ok_or("otel_endpoint is required")?,
            otel_protocol: self.otel_protocol,
        })
    }
}

#[cfg(test)]
mod tests {
    use temp_env::with_vars;

    use super::*;

    #[test]
    fn test_traces_enabled() {
        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("trace_endpoint")),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("general_endpoint")),
            ],
            || {
                assert!(Config::traces_enabled());
            },
        );

        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("")),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("general_endpoint")),
            ],
            || {
                assert!(Config::traces_enabled());
            },
        );

        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, None),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("general_endpoint")),
            ],
            || {
                assert!(Config::traces_enabled());
            },
        );

        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("trace_endpoint")),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("")),
            ],
            || {
                assert!(Config::traces_enabled());
            },
        );

        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("")),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("")),
            ],
            || {
                assert!(!Config::traces_enabled());
            },
        );

        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, None::<&str>),
                (OTEL_EXPORTER_OTLP_ENDPOINT, None::<&str>),
            ],
            || {
                assert!(!Config::traces_enabled());
            },
        );
    }

    #[test]
    fn test_get_empty_trace_context() {
        let trace_context = Config::get_trace_context();
        assert!(trace_context.is_ok());

        let trace_context = trace_context.unwrap();
        assert_eq!(trace_context, "{}");
    }

    #[test]
    fn test_set_empty_trace_context() {
        let trace_context = Config::set_trace_context("{}");
        assert!(trace_context.is_ok());
    }

    #[test]
    fn test_otel_endpoint_from_env() {
        with_vars(
            [(OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("trace_endpoint"))],
            || {
                let builder = ConfigBuilder::default().otel_endpoint_from_env();
                assert_eq!(builder.otel_endpoint, Some("trace_endpoint".to_string()));
            },
        );
    }

    #[test]
    fn test_otel_endpoint_from_env_fallback() {
        with_vars(
            [(OTEL_EXPORTER_OTLP_ENDPOINT, Some("fallback_endpoint"))],
            || {
                let builder = ConfigBuilder::default().otel_endpoint_from_env();
                assert_eq!(builder.otel_endpoint, Some("fallback_endpoint".to_string()));
            },
        );
    }

    #[test]
    fn test_otel_endpoint_from_env_missing() {
        let builder = ConfigBuilder::default().otel_endpoint_from_env();
        assert_eq!(builder.otel_endpoint, None);
    }

    #[test]
    fn test_otel_protocol_from_env() {
        with_vars(
            [(OTEL_EXPORTER_OTLP_TRACES_PROTOCOL, Some("trace_protocol"))],
            || {
                let builder = ConfigBuilder::default().otel_protocol_from_env();
                assert_eq!(builder.otel_protocol, "trace_protocol");
            },
        );
    }

    #[test]
    fn test_otel_protocol_from_env_fallback() {
        with_vars(
            [(OTEL_EXPORTER_OTLP_PROTOCOL, Some("fallback_protocol"))],
            || {
                let builder = ConfigBuilder::default().otel_protocol_from_env();
                assert_eq!(builder.otel_protocol, "fallback_protocol");
            },
        );
    }

    #[test]
    fn test_otel_protocol_from_env_default() {
        let builder = ConfigBuilder::default().otel_protocol_from_env();
        assert_eq!(
            builder.otel_protocol,
            OTEL_EXPORTER_OTLP_PROTOCOL_DEFAULT.to_string()
        );
    }

    #[test]
    fn test_build_with_both_specific_and_general_env_vars() {
        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("trace_endpoint")),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("general_endpoint")),
                (OTEL_EXPORTER_OTLP_TRACES_PROTOCOL, Some("trace_protocol")),
                (OTEL_EXPORTER_OTLP_PROTOCOL, Some("general_protocol")),
            ],
            || {
                let builder = ConfigBuilder::default()
                    .otel_endpoint_from_env()
                    .otel_protocol_from_env();
                let config = builder.build().unwrap();
                assert_eq!(config.otel_endpoint, "trace_endpoint".to_string());
                assert_eq!(config.otel_protocol, "trace_protocol".to_string());
            },
        );
    }

    #[test]
    fn test_build_missing_endpoint() {
        let builder = ConfigBuilder::default().otel_protocol_from_env();
        let result = builder.build();
        assert!(result.is_err());
        assert_eq!(result.err(), Some("otel_endpoint is required"));
    }
}
