//! OpenTelemetry Configuration Module
//!
//! This module provides a configuration structure and associated methods to initialize
//! OpenTelemetry tracing with the OTLP exporter. The configuration can be set up via
//! the `Config` struct and its builder pattern.
//!
//! # Usage
//!
//! ```rust,ignore
//! # // OtlpConfig and otel_traces_enabled are private to the crate
//! use crate::sandbox::shim::OtlpConfig;
//! use crate::sandbox::shim::otel_traces_enabled;
//!
//! fn main() -> anyhow::Result<()> {
//!     if otel_traces_enabled() {
//!         let otel_config = OtlpConfig::build_from_env()?;
//!    
//!         let _guard = otel_config.init()?;
//!    
//!         // Your application code here
//!     }
//!     Ok(())
//! }
//! ```

use std::collections::HashMap;
use std::env;

use opentelemetry::Context;
use opentelemetry::global::{self, set_text_map_propagator};
use opentelemetry::propagation::Extractor;
use opentelemetry::trace::TraceError;
pub use opentelemetry_otlp::{
    OTEL_EXPORTER_OTLP_ENDPOINT, OTEL_EXPORTER_OTLP_PROTOCOL, OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
};
use opentelemetry_otlp::{
    OTEL_EXPORTER_OTLP_PROTOCOL_DEFAULT, Protocol, SpanExporterBuilder, WithExportConfig,
};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::{runtime, trace as sdktrace};
use tracing::span::{Attributes, Id};
use tracing::{Span, Subscriber};
use tracing_opentelemetry::{OpenTelemetrySpanExt as _, OtelData};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::{EnvFilter, Layer, Registry};

const OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_JSON: &str = "http/json";
const OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF: &str = "http/protobuf";
const OTEL_EXPORTER_OTLP_PROTOCOL_GRPC: &str = "grpc";
const OTEL_EXPORTER_OTLP_TRACES_PROTOCOL: &str = "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL";
const OTEL_SDK_DISABLED: &str = "OTEL_SDK_DISABLED";

/// Configuration struct for OpenTelemetry setup.
pub struct Config {
    traces_endpoint: String,
    traces_protocol: Protocol,
}

/// Returns `true` if traces are enabled, `false` otherwise.
///
/// Traces are enabled if either `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` or `OTEL_EXPORTER_OTLP_ENDPOINT` is set and not empty.
/// `OTEL_SDK_DISABLED` can be set to `true` to disable traces.
pub fn traces_enabled() -> bool {
    let check_env_var = |var: &str| env::var_os(var).is_some_and(|val| !val.is_empty());
    let traces_endpoint = check_env_var(OTEL_EXPORTER_OTLP_TRACES_ENDPOINT);
    let otlp_endpoint = check_env_var(OTEL_EXPORTER_OTLP_ENDPOINT);

    // https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/#general-sdk-configuration
    let sdk_disabled = env::var_os(OTEL_SDK_DISABLED).is_some_and(|val| val == "true");
    (traces_endpoint || otlp_endpoint) && !sdk_disabled
}

/// Initializes a new OpenTelemetry tracer with the OTLP exporter.
///
/// Returns a `Result` containing the initialized tracer or a `TraceError` if initialization fails.
///
/// <https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/protocol/exporter.md#configuration-options>
impl Config {
    pub fn build_from_env() -> anyhow::Result<Self> {
        let traces_endpoint = traces_endpoint_from_env()?;
        let traces_protocol: Protocol = traces_protocol_from_env()?;
        Ok(Self {
            traces_endpoint,
            traces_protocol,
        })
    }

    /// Initializes the tracer, sets up the telemetry and subscriber layers, and sets the global subscriber.
    ///
    /// Note: this function should be called only once and be called by the binary entry point.
    pub fn init(&self) -> anyhow::Result<impl Drop> {
        let tracer = self.init_tracer()?;
        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
        set_text_map_propagator(TraceContextPropagator::new());

        let filter = EnvFilter::try_new("info,h2=off")?;

        let subscriber = Registry::default()
            .with(telemetry)
            .with(filter)
            .with(SpanNamingLayer);

        tracing::subscriber::set_global_default(subscriber)?;
        Ok(ShutdownGuard)
    }

    /// Returns the current trace context as a JSON string.
    pub fn get_trace_context() -> anyhow::Result<String> {
        // propagate the context
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

    fn init_tracer_http(&self) -> SpanExporterBuilder {
        opentelemetry_otlp::new_exporter()
            .http()
            .with_endpoint(&self.traces_endpoint)
            .into()
    }

    fn init_tracer_grpc(&self) -> SpanExporterBuilder {
        opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint(&self.traces_endpoint)
            .into()
    }

    fn init_tracer(&self) -> Result<opentelemetry_sdk::trace::Tracer, TraceError> {
        let exporter = match self.traces_protocol {
            Protocol::HttpBinary => self.init_tracer_http(),
            Protocol::HttpJson => self.init_tracer_http(),
            Protocol::Grpc => self.init_tracer_grpc(),
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
struct ShutdownGuard;

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        // Give tracer provider a chance to flush any pending traces.
        opentelemetry::global::shutdown_tracer_provider();
    }
}

/// Sets the OTLP endpoint from environment variables.
fn traces_endpoint_from_env() -> anyhow::Result<String> {
    Ok(env::var(OTEL_EXPORTER_OTLP_TRACES_ENDPOINT)
        .or_else(|_| env::var(OTEL_EXPORTER_OTLP_ENDPOINT))?)
}

/// Sets the OTLP protocol from environment variables.
fn traces_protocol_from_env() -> anyhow::Result<Protocol> {
    let traces_protocol = env::var(OTEL_EXPORTER_OTLP_TRACES_PROTOCOL).unwrap_or(
        env::var(OTEL_EXPORTER_OTLP_PROTOCOL)
            .unwrap_or(OTEL_EXPORTER_OTLP_PROTOCOL_DEFAULT.to_owned()),
    );
    let protocol = match traces_protocol.as_str() {
        OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF => Protocol::HttpBinary,
        OTEL_EXPORTER_OTLP_PROTOCOL_GRPC => Protocol::Grpc,
        OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_JSON => Protocol::HttpJson,
        _ => Err(TraceError::from(
            "Invalid OTEL_EXPORTER_OTLP_PROTOCOL value",
        ))?,
    };
    Ok(protocol)
}

/// A layer that renames spans to include the target in the span name.
struct SpanNamingLayer;

impl<S> Layer<S> for SpanNamingLayer
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &Attributes<'_>,
        id: &Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let span = ctx.span(id).expect("Span not found");
        let mut extensions = span.extensions_mut();

        if let Some(otel_data) = extensions.get_mut::<OtelData>() {
            let target = attrs.metadata().target();
            let original_name = attrs.metadata().name();
            let new_name = format!("{}::{}", target, original_name);
            otel_data.builder.name = new_name.into();
        }
    }
}

/// MetadataExtractor is a wrapper around `HashMap<String, Vec<String>>` which is the type
/// as TtrpcContext.meatdata. It implements the Extractor trait from opentelemetry.
struct MetadataExtractor<'a>(pub &'a HashMap<String, Vec<String>>);

impl Extractor for MetadataExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.first()).map(|s| s.as_str())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

/// extract_context extracts the context from the metadata HashMap.
pub(crate) fn extract_context(metadata: &HashMap<String, Vec<String>>) -> Context {
    let extractor = MetadataExtractor(metadata);
    opentelemetry::global::get_text_map_propagator(|propagator| propagator.extract(&extractor))
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
                (OTEL_SDK_DISABLED, None::<&str>),
            ],
            || {
                assert!(traces_enabled());
            },
        );

        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("")),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("general_endpoint")),
                (OTEL_SDK_DISABLED, Some("t")),
            ],
            || {
                assert!(traces_enabled());
            },
        );

        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, None),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("general_endpoint")),
                (OTEL_SDK_DISABLED, Some("false")),
            ],
            || {
                assert!(traces_enabled());
            },
        );

        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("trace_endpoint")),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("")),
                (OTEL_SDK_DISABLED, Some("1")),
            ],
            || {
                assert!(traces_enabled());
            },
        );

        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("")),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("")),
                (OTEL_SDK_DISABLED, None::<&str>),
            ],
            || {
                assert!(!traces_enabled());
            },
        );

        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, None::<&str>),
                (OTEL_EXPORTER_OTLP_ENDPOINT, None::<&str>),
                (OTEL_SDK_DISABLED, None::<&str>),
            ],
            || {
                assert!(!traces_enabled());
            },
        );

        // Test when traces are disabled due to OTEL_SDK_DISABLED
        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("trace_endpoint")),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("general_endpoint")),
                (OTEL_SDK_DISABLED, Some("true")),
            ],
            || {
                assert!(!traces_enabled());
            },
        );
    }

    #[test]
    fn test_get_empty_trace_context() {
        with_vars::<String, &str, _, _>([], || {
            let trace_context = Config::get_trace_context();
            assert!(trace_context.is_ok());

            let trace_context = trace_context.unwrap();
            assert_eq!(trace_context, "{}");
        });
    }

    #[test]
    fn test_set_empty_trace_context() {
        with_vars::<String, &str, _, _>([], || {
            let trace_context = Config::set_trace_context("{}");
            assert!(trace_context.is_ok());
        });
    }

    #[test]
    fn test_otel_endpoint_from_env() {
        with_vars(
            [(OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("trace_endpoint"))],
            || {
                let result = traces_endpoint_from_env();
                assert!(result.is_ok());
                assert_eq!(result.unwrap(), "trace_endpoint".to_owned());
            },
        );
    }

    #[test]
    fn test_otel_endpoint_from_env_fallback() {
        with_vars(
            [(OTEL_EXPORTER_OTLP_ENDPOINT, Some("fallback_endpoint"))],
            || {
                let result = traces_endpoint_from_env();
                assert!(result.is_ok());
                assert_eq!(result.unwrap(), "fallback_endpoint".to_string());
            },
        );
    }

    #[test]
    fn test_otel_endpoint_from_env_missing() {
        with_vars::<String, &str, _, _>([], || {
            let result = traces_endpoint_from_env();
            assert!(result.is_err());
        });
    }

    #[test]
    fn test_otel_protocol_from_env() {
        with_vars([(OTEL_EXPORTER_OTLP_TRACES_PROTOCOL, Some("grpc"))], || {
            let result = traces_protocol_from_env();
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), Protocol::Grpc);
        });
    }

    #[test]
    fn test_otel_protocol_from_env_fail() {
        with_vars(
            [(OTEL_EXPORTER_OTLP_PROTOCOL, Some("something-else"))],
            || {
                let result = traces_protocol_from_env();
                assert!(result.is_err());
            },
        );
    }

    #[test]
    fn test_otel_protocol_from_env_default() {
        with_vars::<String, &str, _, _>([], || {
            let result = traces_protocol_from_env();
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), Protocol::HttpBinary);
        });
    }

    #[test]
    fn test_build_with_both_specific_and_general_env_vars() {
        with_vars(
            [
                (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, Some("trace_endpoint")),
                (OTEL_EXPORTER_OTLP_ENDPOINT, Some("general_endpoint")),
                (OTEL_EXPORTER_OTLP_TRACES_PROTOCOL, Some("grpc")),
                (OTEL_EXPORTER_OTLP_PROTOCOL, Some("http/protobuf")),
            ],
            || {
                let config = Config::build_from_env().unwrap();
                assert_eq!(config.traces_endpoint, "trace_endpoint".to_string());
                assert_eq!(config.traces_protocol, Protocol::Grpc);
            },
        );
    }

    #[test]
    fn test_build_missing_endpoint() {
        with_vars::<String, &str, _, _>([], || {
            let result = Config::build_from_env();
            assert!(result.is_err());
        });
    }

    #[test]
    fn test_metadata_extractor() {
        let mut metadata = HashMap::new();
        metadata.insert("key".to_string(), vec!["value".to_string()]);
        metadata.insert("key2".to_string(), vec!["value2".to_string()]);

        let extractor = MetadataExtractor(&metadata);
        assert_eq!(extractor.get("key"), Some("value"));
        let mut keys = extractor.keys();
        keys.sort();
        assert_eq!(keys, vec!["key", "key2"]);

        assert_eq!(extractor.get("key2"), Some("value2"));
    }
}
