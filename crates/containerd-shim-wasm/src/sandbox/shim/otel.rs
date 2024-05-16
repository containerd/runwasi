//! OpenTelemetry Configuration Module
//!
//! This module provides a configuration structure and associated methods to initialize
//! OpenTelemetry tracing with the OTLP exporter. The configuration can be set up via
//! the `OtelConfig` struct and its builder pattern.
//!
//! # Usage
//!
//! ```rust
//! use containerd-shim-wasm::sandbox::shim::otel::{OtelConfig, OTEL_EXPORTER_OTLP_ENDPOINT};
//!
//! fn main() -> anyhow::Result<()> {
//!     let otel_endpoint = std::env::var(OTEL_EXPORTER_OTLP_ENDPOINT).expect("OTEL_EXPORTER_OTLP_ENDPOINT not set");
//!     let otel_config = OtelConfig::builder()
//!         .otel_endpoint(otel_endpoint)
//!         .name("my-service".to_string())
//!         .build()?;
//!
//!     let _guard = otel_config.init()?;
//!
//!     // Your application code here
//!
//!     Ok(())
//! }
//! ```

use opentelemetry::global::set_text_map_propagator;
use opentelemetry::trace::TraceError;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::{runtime, trace as sdktrace, Resource};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::{EnvFilter, Registry};

pub const OTEL_EXPORTER_OTLP_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";

/// Configuration struct for OpenTelemetry setup.
pub struct OtelConfig {
    otel_endpoint: String,
    name: String,
}

impl OtelConfig {
    /// Creates a new builder for `OtelConfig`.
    pub fn builder() -> OtelConfigBuilder {
        OtelConfigBuilder::default()
    }

    /// Initializes a new OpenTelemetry tracer with the OTLP exporter.
    ///
    /// Returns a `Result` containing the initialized tracer or a `TraceError` if initialization fails.
    ///
    /// https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/protocol/exporter.md#configuration-options
    fn init_tracer(&self) -> Result<opentelemetry_sdk::trace::Tracer, TraceError> {
        opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(
                opentelemetry_otlp::new_exporter()
                    .tonic()
                    .with_endpoint(&self.otel_endpoint),
            )
            .with_trace_config(sdktrace::config().with_resource(Resource::new(vec![
                KeyValue::new("service.name", format!("containerd-shim-{}", self.name)),
            ])))
            .install_batch(runtime::Tokio)
    }

    /// Initializes the tracer, sets up the telemetry and subscriber layers, and sets the global subscriber.
    pub fn init(&self) -> anyhow::Result<ShutdownGuard> {
        let tracer = self.init_tracer()?;
        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
        set_text_map_propagator(TraceContextPropagator::new());

        let filter = EnvFilter::try_new("info,h2=off")?;

        let subscriber = Registry::default().with(telemetry).with(filter);

        tracing::subscriber::set_global_default(subscriber)?;
        Ok(ShutdownGuard)
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
pub struct OtelConfigBuilder {
    otel_endpoint: Option<String>,
    name: Option<String>,
}

impl OtelConfigBuilder {
    /// Sets the OpenTelemetry endpoint.
    pub fn otel_endpoint(mut self, otel_endpoint: String) -> Self {
        self.otel_endpoint = Some(otel_endpoint);
        self
    }

    /// Sets the service name.
    pub fn name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }

    /// Builds the `OtelConfig` instance.
    pub fn build(self) -> Result<OtelConfig, &'static str> {
        let otel_endpoint = self.otel_endpoint.ok_or("otel_endpoint is required")?;
        let name = self.name.ok_or("name is required")?;
        Ok(OtelConfig {
            otel_endpoint,
            name,
        })
    }
}
