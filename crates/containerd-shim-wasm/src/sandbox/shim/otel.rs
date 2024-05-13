use opentelemetry::trace::TraceError;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{runtime, trace as sdktrace, Resource};

/// Initialize a new OpenTelemetry tracer with the OTLP exporter.
/// The `otel_endpoint` is the endpoint passed down from the
/// environment variable `OTEL_EXPORTER_OTLP_ENDPOINT` from Containerd.
///
/// The `name` is the name of the service that will be used as a resource.
///
/// https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/protocol/exporter.md#configuration-options
pub fn init_tracer(
    otel_endpoint: &str,
    name: &str,
) -> Result<opentelemetry_sdk::trace::Tracer, TraceError> {
    opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .tonic()
                .with_endpoint(otel_endpoint),
        )
        .with_trace_config(
            sdktrace::config().with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                format!("containerd-shim-{}", name),
            )])),
        )
        .install_batch(runtime::Tokio)
}
