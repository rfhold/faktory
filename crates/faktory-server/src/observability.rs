//! Bounded local logging and optional OTLP traces and metrics.

use std::{collections::BTreeMap, env, fmt, io, time::Duration};

use chrono::{SecondsFormat, Utc};
use opentelemetry::{
    KeyValue, global,
    trace::{TraceContextExt as _, TracerProvider as _},
};
use opentelemetry_otlp::{Protocol, WithExportConfig as _};
use opentelemetry_sdk::{
    Resource,
    metrics::SdkMeterProvider,
    propagation::TraceContextPropagator,
    trace::{Sampler, SdkTracerProvider},
};
use serde_json::Value;
use tracing::{Event, Level, Metadata, Subscriber, field::Visit};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::{
    EnvFilter, Layer as _,
    filter::{FilterExt as _, filter_fn},
    fmt::{FmtContext, FormatEvent, FormatFields, format::Writer},
    layer::SubscriberExt as _,
    registry::LookupSpan,
    util::SubscriberInitExt as _,
};
use url::Url;

pub const SERVICE_NAME: &str = "faktory";
pub const SERVICE_NAMESPACE: &str = "faktory";
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelemetryConfig {
    pub deployment_environment: String,
    pub k8s_namespace: Option<String>,
    pub k8s_pod_name: Option<String>,
    pub k8s_pod_uid: Option<String>,
    pub pyroscope_url: Option<Url>,
}

impl TelemetryConfig {
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            deployment_environment: required("FAKTORY_DEPLOYMENT_ENVIRONMENT")?,
            k8s_namespace: optional("FAKTORY_K8S_NAMESPACE"),
            k8s_pod_name: optional("FAKTORY_K8S_POD_NAME"),
            k8s_pod_uid: optional("FAKTORY_K8S_POD_UID"),
            pyroscope_url: optional("FAKTORY_PYROSCOPE_URL")
                .map(|value| secure_origin("FAKTORY_PYROSCOPE_URL", &value))
                .transpose()?,
        })
    }
}

#[derive(Debug)]
pub struct ObservabilityGuard {
    tracer: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
}

impl ObservabilityGuard {
    pub fn shutdown(mut self) {
        tracing::info!("observability shutdown started");
        if let Some(meter) = self.meter.take()
            && meter.shutdown_with_timeout(SHUTDOWN_TIMEOUT).is_err()
        {
            tracing::warn!("failed to shut down meter provider");
        }
        if let Some(tracer) = self.tracer.take()
            && tracer.shutdown_with_timeout(SHUTDOWN_TIMEOUT).is_err()
        {
            tracing::warn!("failed to shut down tracer provider");
        }
        tracing::info!("observability shutdown complete");
    }
}

impl Drop for ObservabilityGuard {
    fn drop(&mut self) {
        if let Some(meter) = self.meter.take() {
            drop(meter.shutdown_with_timeout(SHUTDOWN_TIMEOUT));
        }
        if let Some(tracer) = self.tracer.take() {
            drop(tracer.shutdown_with_timeout(SHUTDOWN_TIMEOUT));
        }
    }
}

pub fn init(
    config: &TelemetryConfig,
) -> Result<ObservabilityGuard, Box<dyn std::error::Error + Send + Sync>> {
    let export_mode = otlp_export_mode().map_err(io::Error::other)?;
    global::set_text_map_propagator(TraceContextPropagator::new());

    if export_mode == OtlpExportMode::Local {
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(JsonEventFormatter)
                    .with_filter(json_filter()),
            )
            .try_init()?;
        tracing::info!(
            service.name = SERVICE_NAME,
            service.namespace = SERVICE_NAMESPACE,
            deployment.environment.name = %config.deployment_environment,
            otlp.enabled = false,
            "observability initialized"
        );
        return Ok(ObservabilityGuard {
            tracer: None,
            meter: None,
        });
    }

    let resource = resource(config);
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .build()
        .map_err(|_| io::Error::other("failed to initialize OTLP trace exporter"))?;
    let tracer_provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .with_resource(resource.clone())
        .with_batch_exporter(span_exporter)
        .build();
    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .build()
        .map_err(|_| io::Error::other("failed to initialize OTLP metric exporter"))?;
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_periodic_exporter(metric_exporter)
        .build();

    global::set_tracer_provider(tracer_provider.clone());
    global::set_meter_provider(meter_provider.clone());
    let tracer = tracer_provider.tracer(SERVICE_NAME);
    tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .with(
            tracing_subscriber::fmt::layer()
                .event_format(JsonEventFormatter)
                .with_filter(json_filter()),
        )
        .with(trace_filter())
        .try_init()?;

    tracing::info!(
        service.name = SERVICE_NAME,
        service.namespace = SERVICE_NAMESPACE,
        deployment.environment.name = %config.deployment_environment,
        otlp.enabled = true,
        otel.traces.sampler = "always_on",
        "observability initialized"
    );
    Ok(ObservabilityGuard {
        tracer: Some(tracer_provider),
        meter: Some(meter_provider),
    })
}

fn resource(config: &TelemetryConfig) -> Resource {
    let mut attributes = vec![
        KeyValue::new("service.name", SERVICE_NAME),
        KeyValue::new("service.namespace", SERVICE_NAMESPACE),
        KeyValue::new(
            "deployment.environment.name",
            config.deployment_environment.clone(),
        ),
    ];
    for (key, value) in [
        ("k8s.namespace.name", &config.k8s_namespace),
        ("k8s.pod.name", &config.k8s_pod_name),
        ("k8s.pod.uid", &config.k8s_pod_uid),
        ("service.instance.id", &config.k8s_pod_uid),
    ] {
        if let Some(value) = value {
            attributes.push(KeyValue::new(key, value.clone()));
        }
    }
    Resource::builder().with_attributes(attributes).build()
}

fn configured_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("faktory_server=info"))
}

fn allowed_target(target: &str) -> bool {
    target == "faktory_server" || target.starts_with("faktory_server::")
}

fn telemetry_metadata_allowed(metadata: &Metadata<'_>) -> bool {
    allowed_target(metadata.target())
        && matches!(*metadata.level(), Level::ERROR | Level::WARN | Level::INFO)
}

pub(crate) fn json_filter<S>() -> impl tracing_subscriber::layer::Filter<S>
where
    S: Subscriber,
{
    filter_fn(telemetry_metadata_allowed).and(configured_env_filter())
}

pub(crate) fn trace_filter<S>() -> impl tracing_subscriber::Layer<S>
where
    S: Subscriber,
{
    filter_fn(telemetry_metadata_allowed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OtlpExportMode {
    Local,
    Export,
}

fn otlp_export_mode() -> Result<OtlpExportMode, &'static str> {
    otlp_export_mode_with(|name| env::var_os(name).is_some())
}

fn otlp_export_mode_with(
    mut is_set: impl FnMut(&str) -> bool,
) -> Result<OtlpExportMode, &'static str> {
    if is_set("OTEL_EXPORTER_OTLP_ENDPOINT") {
        return Ok(OtlpExportMode::Export);
    }
    match (
        is_set("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"),
        is_set("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT"),
    ) {
        (false, false) => Ok(OtlpExportMode::Local),
        (true, true) => Ok(OtlpExportMode::Export),
        _ => Err("invalid OTLP endpoint configuration"),
    }
}

fn required(name: &str) -> Result<String, String> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

fn optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn secure_origin(name: &str, value: &str) -> Result<Url, String> {
    let url =
        Url::parse(value).map_err(|_| format!("{name} must be a credential-free HTTPS root"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!("{name} must be a credential-free HTTPS root"));
    }
    Ok(url)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct JsonEventFormatter;

impl<S, N> FormatEvent<S, N> for JsonEventFormatter
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        _context: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        let mut fields = JsonFields::default();
        event.record(&mut fields);
        let mut object = serde_json::Map::new();
        object.insert(
            "timestamp".to_owned(),
            Value::String(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
        );
        object.insert(
            "level".to_owned(),
            Value::String(metadata.level().to_string()),
        );
        object.insert(
            "target".to_owned(),
            Value::String(metadata.target().to_owned()),
        );
        for (key, value) in fields.0 {
            object.insert(key, value);
        }
        insert_trace_correlation(&mut object);
        let line = serde_json::to_string(&object).map_err(|_| fmt::Error)?;
        writeln!(writer, "{line}")
    }
}

fn insert_trace_correlation(object: &mut serde_json::Map<String, Value>) {
    let context = tracing::Span::current().context();
    let span = context.span();
    let span_context = span.span_context();
    if span_context.is_valid() {
        object.insert(
            "trace_id".to_owned(),
            Value::String(span_context.trace_id().to_string()),
        );
        object.insert(
            "span_id".to_owned(),
            Value::String(span_context.span_id().to_string()),
        );
    }
}

#[derive(Default)]
struct JsonFields(BTreeMap<String, Value>);

impl Visit for JsonFields {
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.insert(field.name().to_owned(), Value::Bool(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.0.insert(
            field.name().to_owned(),
            serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number),
        );
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0
            .insert(field.name().to_owned(), Value::String(value.to_owned()));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        self.0
            .insert(field.name().to_owned(), Value::String(format!("{value:?}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_mode_requires_shared_or_both_signal_endpoints() {
        let mode = |set: &[&str]| otlp_export_mode_with(|name| set.contains(&name));
        assert_eq!(mode(&[]), Ok(OtlpExportMode::Local));
        assert_eq!(
            mode(&["OTEL_EXPORTER_OTLP_ENDPOINT"]),
            Ok(OtlpExportMode::Export)
        );
        assert_eq!(
            mode(&[
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
            ]),
            Ok(OtlpExportMode::Export)
        );
        for endpoint in [
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        ] {
            assert_eq!(
                mode(&[endpoint]),
                Err("invalid OTLP endpoint configuration")
            );
        }
    }

    #[test]
    fn target_and_level_allowlist_is_bounded() {
        for target in ["faktory_server", "faktory_server::app"] {
            assert!(allowed_target(target));
        }
        for target in [
            "mcp",
            "tower_http",
            "opentelemetry_otlp",
            "faktory_server_evil",
        ] {
            assert!(!allowed_target(target));
        }
    }

    #[test]
    fn resource_contains_approved_identity() {
        let config = test_config();
        let resource = resource(&config);
        for (key, expected) in [
            ("service.name", SERVICE_NAME),
            ("service.namespace", SERVICE_NAMESPACE),
            ("deployment.environment.name", "preview"),
            ("k8s.namespace.name", "faktory-system"),
            ("k8s.pod.name", "faktory-abc"),
            ("k8s.pod.uid", "pod-uid"),
            ("service.instance.id", "pod-uid"),
        ] {
            assert_eq!(
                resource
                    .get(&opentelemetry::Key::new(key))
                    .expect("resource attribute")
                    .to_string(),
                expected
            );
        }
    }

    #[test]
    fn pyroscope_origin_rejects_credentials_and_non_root_urls() {
        assert!(secure_origin("url", "https://profiles.example.com/").is_ok());
        for url in [
            "http://profiles.example.com/",
            "https://user:secret@profiles.example.com/",
            "https://profiles.example.com/path",
            "https://profiles.example.com/?token=secret",
            "https://profiles.example.com/#fragment",
        ] {
            assert!(secure_origin("url", url).is_err(), "accepted {url}");
        }
    }

    fn test_config() -> TelemetryConfig {
        TelemetryConfig {
            deployment_environment: "preview".to_owned(),
            k8s_namespace: Some("faktory-system".to_owned()),
            k8s_pod_name: Some("faktory-abc".to_owned()),
            k8s_pod_uid: Some("pod-uid".to_owned()),
            pyroscope_url: None,
        }
    }
}
