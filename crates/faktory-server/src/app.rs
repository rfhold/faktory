//! Same-origin Axum, Tonic gRPC-web, artifact, health, and MCP composition.

use std::{
    convert::Infallible,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

use axum::{
    Router,
    body::Body,
    extract::{MatchedPath, Path, State},
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse as _, Response},
    routing::{get, get_service},
};
use faktory_proto::v1::faktory_service_server::FaktoryServiceServer;
use kuri_server::access_auth::{
    GrpcAccessAuthLayer, GrpcTransportLayer, access_auth_router, require_browser_session,
};
use opentelemetry::{KeyValue, global};
use tonic::service::Routes;
use tower::{Layer, Service, ServiceBuilder};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::Instrument as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::{
    auth::AuthConfig,
    mcp::FaktoryMcp,
    model::{Repository, RepositoryError},
    production::ProductionAuthRuntime,
    render::{RenderConfig, RenderQueue},
    service::FaktoryGrpcService,
    storage::ObjectStore,
};

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub auth: AuthConfig,
    pub public_base_url: String,
    pub static_directory: Option<PathBuf>,
    pub render: RenderConfig,
    pub watch_capacity: usize,
}

#[derive(Clone, Debug)]
struct AppState {
    repository: Repository,
    readiness: AuthReadiness,
}

#[derive(Clone, Debug)]
enum AuthReadiness {
    Disabled,
    Production(Arc<ProductionAuthRuntime>),
}

#[derive(Clone, Debug)]
pub struct Runtime {
    router: Router,
    repository: Repository,
    renders: RenderQueue,
}

#[derive(Clone)]
struct HttpMetrics {
    requests: opentelemetry::metrics::Counter<u64>,
    active_requests: opentelemetry::metrics::UpDownCounter<i64>,
    duration: opentelemetry::metrics::Histogram<f64>,
    #[cfg(test)]
    active_balance: Arc<std::sync::atomic::AtomicI64>,
    #[cfg(test)]
    completed_requests: Arc<std::sync::atomic::AtomicU64>,
}

impl HttpMetrics {
    fn new() -> Self {
        Self::from_meter(global::meter("faktory.http"))
    }

    fn from_meter(meter: opentelemetry::metrics::Meter) -> Self {
        Self {
            requests: meter
                .u64_counter("http.server.request.count")
                .with_description("Completed HTTP server requests")
                .build(),
            active_requests: meter
                .i64_up_down_counter("http.server.active_requests")
                .with_description("Active HTTP server requests")
                .build(),
            duration: meter
                .f64_histogram("http.server.request.duration")
                .with_unit("s")
                .with_description("HTTP server request duration")
                .build(),
            #[cfg(test)]
            active_balance: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            #[cfg(test)]
            completed_requests: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    fn add_active(&self, value: i64, attributes: &[KeyValue]) {
        self.active_requests.add(value, attributes);
        #[cfg(test)]
        self.active_balance
            .fetch_add(value, std::sync::atomic::Ordering::Relaxed);
    }
}

struct RequestMetricsGuard {
    metrics: HttpMetrics,
    span: tracing::Span,
    method: String,
    route: String,
    started: Instant,
    finished: bool,
}

impl RequestMetricsGuard {
    fn new(metrics: HttpMetrics, span: tracing::Span, method: String, route: String) -> Self {
        metrics.add_active(
            1,
            &[
                KeyValue::new("http.request.method", method.clone()),
                KeyValue::new("http.route", route.clone()),
            ],
        );
        Self {
            metrics,
            span,
            method,
            route,
            started: Instant::now(),
            finished: false,
        }
    }

    fn complete(&mut self, status: StatusCode) {
        let outcome = if status.is_server_error() {
            "error"
        } else {
            "success"
        };
        self.finish(Some(status), outcome);
    }

    fn finish(&mut self, status: Option<StatusCode>, outcome: &'static str) {
        if self.finished {
            return;
        }
        self.finished = true;
        let elapsed = self.started.elapsed().as_secs_f64();
        self.span.record("http.outcome", outcome);
        if let Some(status) = status {
            self.span
                .record("http.response.status_code", status.as_u16());
            if status.is_server_error() {
                self.span.record("otel.status_code", "ERROR");
            }
        } else {
            self.span.record("otel.status_code", "ERROR");
        }

        let active_attributes = [
            KeyValue::new("http.request.method", self.method.clone()),
            KeyValue::new("http.route", self.route.clone()),
        ];
        let mut completed_attributes = vec![
            KeyValue::new("http.request.method", self.method.clone()),
            KeyValue::new("http.route", self.route.clone()),
            KeyValue::new("http.outcome", outcome),
        ];
        if let Some(status) = status {
            completed_attributes.push(KeyValue::new(
                "http.response.status_code",
                i64::from(status.as_u16()),
            ));
        }
        self.metrics.add_active(-1, &active_attributes);
        self.metrics.requests.add(1, &completed_attributes);
        #[cfg(test)]
        self.metrics
            .completed_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.metrics.duration.record(elapsed, &completed_attributes);
        let _entered = self.span.enter();
        if let Some(status) = status {
            tracing::info!(
                http.request.method = self.method,
                http.route = self.route,
                http.response.status_code = status.as_u16(),
                http.outcome = outcome,
                duration_seconds = elapsed,
                "http request completed"
            );
        } else {
            tracing::info!(
                http.request.method = self.method,
                http.route = self.route,
                http.outcome = outcome,
                duration_seconds = elapsed,
                "http request completed"
            );
        }
    }
}

impl Drop for RequestMetricsGuard {
    fn drop(&mut self) {
        self.finish(None, "cancelled");
    }
}

impl Runtime {
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    #[must_use]
    pub const fn repository(&self) -> &Repository {
        &self.repository
    }

    #[must_use]
    pub const fn render_queue(&self) -> &RenderQueue {
        &self.renders
    }
}

pub async fn build_runtime(
    store: Arc<dyn ObjectStore>,
    config: AppConfig,
) -> Result<Runtime, String> {
    let repository = Repository::new(store, config.watch_capacity);
    let renders = RenderQueue::start(repository.clone(), config.render.clone())
        .map_err(|_| "invalid render configuration".to_owned())?;
    renders
        .reconcile(&repository)
        .await
        .map_err(startup_reconciliation_error)?;

    let router = match &config.auth {
        AuthConfig::Disabled => build_disabled_router(&config, repository.clone(), renders.clone()),
        AuthConfig::Production(production_config) => {
            if production_config.public_base_url.trim_end_matches('/')
                != config.public_base_url.trim_end_matches('/')
            {
                return Err("production public base URLs must match".to_owned());
            }
            let production = Arc::new(ProductionAuthRuntime::initialize(production_config).await?);
            build_production_router(&config, repository.clone(), renders.clone(), production)?
        }
    };

    Ok(Runtime {
        router,
        repository,
        renders,
    })
}

fn startup_reconciliation_error(error: RepositoryError) -> String {
    tracing::error!(
        repository.error.kind = error.kind(),
        "startup reconciliation failed"
    );
    "startup reconciliation failed".to_owned()
}

fn build_disabled_router(
    config: &AppConfig,
    repository: Repository,
    renders: RenderQueue,
) -> Router {
    let mut grpc_routes = Routes::builder();
    grpc_routes.add_service(FaktoryServiceServer::new(FaktoryGrpcService::new(
        repository.clone(),
    )));
    let grpc = ServiceBuilder::new()
        .layer(tonic_web::GrpcWebLayer::new())
        .service(grpc_routes.routes());
    let mcp = FaktoryMcp::new(repository.clone(), renders).router();
    let state = AppState {
        repository,
        readiness: AuthReadiness::Disabled,
    };
    let artifact_state = state.clone();
    let preview_state = state.clone();
    let router = base_router(state)
        .route(
            "/artifacts/{model_id}/{revision}/model.glb",
            get(move |path, headers| geometry(State(artifact_state.clone()), path, headers)),
        )
        .route(
            "/artifacts/{model_id}/{revision}/preview.svg",
            get(move |path, headers| preview(State(preview_state.clone()), path, headers)),
        )
        .merge(mcp);
    let mut router = router;
    if let Some(directory) = &config.static_directory {
        router = router
            .route(
                "/",
                get_service(
                    ServeDir::new(directory.clone()).append_index_html_on_directories(true),
                )
                .fallback_service(grpc.clone()),
            )
            .route(
                "/{*path}",
                get_service(
                    ServeDir::new(directory.clone())
                        .append_index_html_on_directories(true)
                        .fallback(ServeFile::new(directory.join("index.html"))),
                )
                .fallback_service(grpc.clone()),
            );
    }
    with_http_telemetry(router.fallback_service(grpc))
}

fn build_production_router(
    config: &AppConfig,
    repository: Repository,
    renders: RenderQueue,
    production: Arc<ProductionAuthRuntime>,
) -> Result<Router, String> {
    let mut grpc_routes = Routes::builder();
    grpc_routes.add_service(FaktoryServiceServer::new(FaktoryGrpcService::new(
        repository.clone(),
    )));
    let grpc = ServiceBuilder::new()
        .layer(GrpcWebOnlyLayer)
        .layer(GrpcTransportLayer)
        .layer(tonic_web::GrpcWebLayer::new())
        .layer(GrpcAccessAuthLayer::new(production.browser.clone()))
        .service(grpc_routes.routes());
    let browser_auth =
        middleware::from_fn_with_state(production.browser.clone(), require_browser_session);
    let production_config = match &config.auth {
        AuthConfig::Production(config) => config,
        AuthConfig::Disabled => unreachable!("production router mode"),
    };
    let mcp = FaktoryMcp::new(repository.clone(), renders).hosted_router(
        production_config.mcp_resource(),
        production_config.oauth_issuer(),
        production.oauth.clone(),
    )?;
    let state = AppState {
        repository,
        readiness: AuthReadiness::Production(production.clone()),
    };
    let artifact_state = state.clone();
    let preview_state = state.clone();
    let router = base_router(state)
        .route(
            "/artifacts/{model_id}/{revision}/model.glb",
            get(move |path, headers| geometry(State(artifact_state.clone()), path, headers))
                .layer(browser_auth.clone()),
        )
        .route(
            "/artifacts/{model_id}/{revision}/preview.svg",
            get(move |path, headers| preview(State(preview_state.clone()), path, headers))
                .layer(browser_auth.clone()),
        )
        .merge(access_auth_router(production.browser.clone()))
        .merge(production.oauth.router())
        .merge(production.oidc_owner.router())
        .merge(mcp);
    let mut router = router;
    if let Some(directory) = &config.static_directory {
        router = router
            .route(
                "/",
                get_service(
                    ServeDir::new(directory.clone()).append_index_html_on_directories(true),
                )
                .layer(browser_auth.clone())
                .fallback_service(grpc.clone()),
            )
            .route(
                "/{*path}",
                get_service(
                    ServeDir::new(directory.clone())
                        .append_index_html_on_directories(true)
                        .fallback(ServeFile::new(directory.join("index.html"))),
                )
                .layer(browser_auth)
                .fallback_service(grpc.clone()),
            );
    }
    Ok(with_http_telemetry(router.fallback_service(grpc)))
}

fn with_http_telemetry(router: Router) -> Router {
    let metrics = HttpMetrics::new();
    router
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(move |request, next| {
            instrument_request(request, next, metrics.clone())
        }))
}

async fn instrument_request(request: Request<Body>, next: Next, metrics: HttpMetrics) -> Response {
    if matches!(request.uri().path(), "/health" | "/ready") {
        return next.run(request).await;
    }

    let method = bounded_http_method(request.method()).to_owned();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(
            || stable_path_class(request.uri().path()),
            MatchedPath::as_str,
        )
        .to_owned();
    let parent = global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(request.headers()))
    });
    let span = tracing::info_span!(
        "http.server.request",
        http.request.method = %method,
        http.route = %route,
        http.response.status_code = tracing::field::Empty,
        http.outcome = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    );
    let _ = span.set_parent(parent);
    let mut metrics_guard = RequestMetricsGuard::new(metrics, span.clone(), method, route);
    let response = next.run(request).instrument(span.clone()).await;
    metrics_guard.complete(response.status());
    response
}

const fn bounded_http_method(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::OPTIONS => "OPTIONS",
        Method::CONNECT => "CONNECT",
        Method::TRACE => "TRACE",
        _ => "OTHER",
    }
}

fn stable_path_class(path: &str) -> &'static str {
    match path {
        "/mcp" => "/mcp",
        _ if path.starts_with("/.well-known/") => "/.well-known/*",
        _ if path.starts_with("/auth/") => "/auth/*",
        _ if path.starts_with("/oauth/") => "/oauth/*",
        _ if path.starts_with("/oidc/") => "/oidc/*",
        _ if path.starts_with("/faktory.v1.FaktoryService/") => "/faktory.v1.FaktoryService/*",
        _ => "unmatched",
    }
}

struct HeaderExtractor<'a>(&'a HeaderMap);

impl opentelemetry::propagation::Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(axum::http::HeaderName::as_str).collect()
    }
}

#[derive(Clone, Copy, Debug)]
struct GrpcWebOnlyLayer;

impl<S> Layer<S> for GrpcWebOnlyLayer {
    type Service = GrpcWebOnlyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        GrpcWebOnlyService { inner }
    }
}

#[derive(Clone, Debug)]
struct GrpcWebOnlyService<S> {
    inner: S,
}

impl<S> Service<http::Request<Body>> for GrpcWebOnlyService<S>
where
    S: Service<
            http::Request<Body>,
            Response = http::Response<tonic::body::Body>,
            Error = Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        if !is_grpc_web(request.headers()) {
            return Box::pin(async {
                Ok(tonic::Status::unauthenticated("gRPC-web browser session required").into_http())
            });
        }
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move { inner.call(request).await })
    }
}

fn is_grpc_web(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            matches!(
                value,
                "application/grpc-web"
                    | "application/grpc-web+proto"
                    | "application/grpc-web-text"
                    | "application/grpc-web-text+proto"
            )
        })
}

fn base_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(move || ready(State(state.clone()))))
}

async fn health() -> impl axum::response::IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        "{\"status\":\"healthy\"}",
    )
}

async fn ready(State(state): State<AppState>) -> Response {
    let auth_ready = match state.readiness {
        AuthReadiness::Disabled => true,
        AuthReadiness::Production(auth) => auth.ready().await,
    };
    if auth_ready && state.repository.ready().await.is_ok() {
        StatusCode::OK.into_response()
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

async fn geometry(
    State(state): State<AppState>,
    Path((model_id, revision)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let bytes = match state.repository.geometry(&model_id, &revision).await {
        Ok(geometry) => geometry,
        Err(RepositoryError::Invalid | RepositoryError::NotFound) => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let etag = format!("\"{revision}\"");
    if if_none_match(&headers, &etag) {
        return response_with_headers(StatusCode::NOT_MODIFIED, Body::empty(), &etag, None, 0);
    }
    let length = bytes.len();
    if let Some(range) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        let Some((start, end)) = parse_range(range, length) else {
            let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            response.headers_mut().insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{length}")).expect("valid content range"),
            );
            return response;
        };
        let body = Body::from(bytes.slice(start..=end));
        return response_with_headers(
            StatusCode::PARTIAL_CONTENT,
            body,
            &etag,
            Some((start, end, length)),
            end - start + 1,
        );
    }
    response_with_headers(StatusCode::OK, Body::from(bytes), &etag, None, length)
}

async fn preview(
    State(state): State<AppState>,
    Path((model_id, revision)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let bytes = match state.repository.preview(&model_id, &revision).await {
        Ok(preview) => preview,
        Err(RepositoryError::Invalid | RepositoryError::NotFound) => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let etag = format!("\"{revision}\"");
    let not_modified = if_none_match(&headers, &etag);
    let (status, body, content_length) = if not_modified {
        (StatusCode::NOT_MODIFIED, Body::empty(), 0)
    } else {
        let length = bytes.len();
        (StatusCode::OK, Body::from(bytes), length)
    };
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/svg+xml"),
    );
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    response_headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).expect("revision etag is valid"),
    );
    response_headers.insert(header::CONTENT_LENGTH, HeaderValue::from(content_length));
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response_headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );
    response
}

fn if_none_match(headers: &HeaderMap, current_etag: &str) -> bool {
    headers.get_all(header::IF_NONE_MATCH).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*"
                    || candidate.strip_prefix("W/").unwrap_or(candidate) == current_etag
            })
        })
    })
}

fn response_with_headers(
    status: StatusCode,
    body: Body,
    etag: &str,
    range: Option<(usize, usize, usize)>,
    content_length: usize,
) -> Response {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("model/gltf-binary"),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(etag).expect("revision etag is valid"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(content_length));
    if let Some((start, end, total)) = range {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{total}"))
                .expect("valid content range"),
        );
    }
    response
}

fn parse_range(value: &str, length: usize) -> Option<(usize, usize)> {
    let value = value.strip_prefix("bytes=")?;
    if value.contains(',') || length == 0 {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<usize>().ok()?.min(length);
        return (suffix > 0).then_some((length - suffix, length - 1));
    }
    let start = start.parse::<usize>().ok()?;
    let end = if end.is_empty() {
        length - 1
    } else {
        end.parse::<usize>().ok()?.min(length - 1)
    };
    (start < length && start <= end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::{BTreeMap, BTreeSet},
        io,
        sync::Mutex,
    };

    use axum::body::to_bytes;
    use opentelemetry::{metrics::MeterProvider as _, trace::TracerProvider as _};
    use opentelemetry_sdk::{
        metrics::{
            InMemoryMetricExporter, SdkMeterProvider,
            data::{AggregatedMetrics, Metric, MetricData},
        },
        trace::{InMemorySpanExporter, SdkTracerProvider},
    };
    use tower::ServiceExt as _;
    use tracing_subscriber::{Layer as _, layer::SubscriberExt as _};

    use crate::storage::InMemoryObjectStore;

    const PRIVATE_SENTINELS: [&str; 5] = [
        "private-model-id",
        "private-query-value",
        "private-authorization-token",
        "private-source-body",
        "private-error-response",
    ];

    fn rendered(glb: &'static [u8]) -> crate::model::RenderedOutput {
        crate::model::RenderedOutput {
            glb: bytes::Bytes::from_static(glb),
            preview: bytes::Bytes::from_static(b"<svg></svg>"),
            facts: crate::model::GeometryFactsRecord {
                volume_cubic_millimeters: 24.0,
                size_millimeters: crate::model::GeometrySizeRecord {
                    x: 2.0,
                    y: 3.0,
                    z: 4.0,
                },
            },
        }
    }

    #[derive(Clone, Debug, Default)]
    struct RecordingWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for RecordingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("recording writer").extend(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn attribute_map<'a>(
        attributes: impl Iterator<Item = &'a KeyValue>,
    ) -> BTreeMap<String, String> {
        attributes
            .map(|attribute| {
                (
                    attribute.key.as_str().to_owned(),
                    attribute.value.to_string(),
                )
            })
            .collect()
    }

    fn metric_attribute_maps(metric: &Metric) -> Vec<BTreeMap<String, String>> {
        match metric.data() {
            AggregatedMetrics::I64(MetricData::Sum(sum)) => sum
                .data_points()
                .map(|point| attribute_map(point.attributes()))
                .collect(),
            AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
                .data_points()
                .map(|point| attribute_map(point.attributes()))
                .collect(),
            AggregatedMetrics::F64(MetricData::Histogram(histogram)) => histogram
                .data_points()
                .map(|point| attribute_map(point.attributes()))
                .collect(),
            data => panic!("unexpected metric data: {data:?}"),
        }
    }

    fn assert_no_private_sentinels(signal: &str, contents: &str) {
        for sentinel in PRIVATE_SENTINELS {
            assert!(!contents.contains(sentinel), "{signal} leaked {sentinel}");
        }
    }

    fn assert_stdout_request_event(output: &RecordingWriter) {
        let stdout = String::from_utf8(output.0.lock().expect("recording writer").clone())
            .expect("UTF-8 stdout");
        assert_no_private_sentinels("stdout", &stdout);
        let events = stdout
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON event"))
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 1, "unexpected stdout events: {stdout}");
        let event = events[0].as_object().expect("event object");
        let allowed_event_fields = BTreeSet::from([
            "duration_seconds",
            "http.outcome",
            "http.request.method",
            "http.response.status_code",
            "http.route",
            "level",
            "message",
            "span_id",
            "target",
            "timestamp",
            "trace_id",
        ]);
        assert!(
            event
                .keys()
                .all(|key| allowed_event_fields.contains(key.as_str()))
        );
        assert_eq!(event["http.request.method"], "POST");
        assert_eq!(event["http.route"], "/models/{model_id}");
        assert_eq!(event["http.outcome"], "error");
        assert_eq!(event["http.response.status_code"], 503);
        assert_eq!(event["message"], "http request completed");
        assert!(event["duration_seconds"].is_number());
    }

    #[test]
    fn startup_reconciliation_logs_only_the_repository_error_kind() {
        let output = RecordingWriter::default();
        let writer = output.clone();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .event_format(crate::observability::JsonEventFormatter)
                .with_writer(move || writer.clone())
                .with_filter(crate::observability::json_filter()),
        );

        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(
                startup_reconciliation_error(RepositoryError::Conflict),
                "startup reconciliation failed"
            );
        });

        let stdout = String::from_utf8(output.0.lock().expect("recording writer").clone())
            .expect("UTF-8 stdout");
        let event = serde_json::from_str::<serde_json::Value>(stdout.trim()).expect("JSON event");
        assert_eq!(event["level"], "ERROR");
        assert_eq!(event["message"], "startup reconciliation failed");
        assert_eq!(event["repository.error.kind"], "conflict");
        assert_eq!(
            event
                .as_object()
                .expect("event object")
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "level",
                "message",
                "repository.error.kind",
                "target",
                "timestamp",
            ])
        );
    }

    fn assert_request_span(span_exporter: &InMemorySpanExporter) {
        let spans = span_exporter.get_finished_spans().expect("exported spans");
        let request_span = spans
            .iter()
            .find(|span| span.name == "http.server.request")
            .expect("HTTP request span");
        assert_no_private_sentinels("span", &format!("{request_span:?}"));
        let http_span_attributes = attribute_map(
            request_span
                .attributes
                .iter()
                .filter(|attribute| attribute.key.as_str().starts_with("http.")),
        );
        assert_eq!(
            http_span_attributes,
            BTreeMap::from([
                ("http.outcome".to_owned(), "error".to_owned()),
                ("http.request.method".to_owned(), "POST".to_owned()),
                ("http.response.status_code".to_owned(), "503".to_owned()),
                ("http.route".to_owned(), "/models/{model_id}".to_owned()),
            ])
        );
    }

    fn assert_request_metrics(metric_exporter: &InMemoryMetricExporter) {
        let exported_metrics = metric_exporter
            .get_finished_metrics()
            .expect("exported metrics");
        assert_no_private_sentinels("metrics", &format!("{exported_metrics:?}"));
        let mut observed_names = BTreeSet::new();
        for metric in exported_metrics
            .iter()
            .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
            .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
        {
            observed_names.insert(metric.name());
            let expected = if metric.name() == "http.server.active_requests" {
                BTreeMap::from([
                    ("http.request.method".to_owned(), "POST".to_owned()),
                    ("http.route".to_owned(), "/models/{model_id}".to_owned()),
                ])
            } else {
                BTreeMap::from([
                    ("http.outcome".to_owned(), "error".to_owned()),
                    ("http.request.method".to_owned(), "POST".to_owned()),
                    ("http.response.status_code".to_owned(), "503".to_owned()),
                    ("http.route".to_owned(), "/models/{model_id}".to_owned()),
                ])
            };
            assert_eq!(
                metric_attribute_maps(metric),
                [expected],
                "attributes for {}",
                metric.name()
            );
        }
        assert_eq!(
            observed_names,
            BTreeSet::from([
                "http.server.active_requests",
                "http.server.request.count",
                "http.server.request.duration",
            ])
        );
    }

    #[test]
    fn parses_single_byte_ranges() {
        assert_eq!(parse_range("bytes=2-5", 10), Some((2, 5)));
        assert_eq!(parse_range("bytes=7-", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=11-12", 10), None);
        assert_eq!(parse_range("bytes=1-2,4-5", 10), None);
    }

    #[test]
    fn telemetry_methods_and_fallback_routes_are_bounded() {
        for (method, expected) in [
            (Method::GET, "GET"),
            (Method::POST, "POST"),
            (Method::from_bytes(b"CUSTOM").expect("method"), "OTHER"),
        ] {
            assert_eq!(bounded_http_method(&method), expected);
        }
        assert_eq!(stable_path_class("/mcp"), "/mcp");
        assert_eq!(
            stable_path_class("/oauth/authorize?code=secret"),
            "/oauth/*"
        );
        assert_eq!(stable_path_class("/models/private-id"), "unmatched");
    }

    #[tokio::test]
    async fn probes_bypass_http_telemetry() {
        let metrics = HttpMetrics::new();
        let router = Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/ready", get(|| async { StatusCode::OK }))
            .route("/normal", get(|| async { StatusCode::NO_CONTENT }))
            .layer(middleware::from_fn({
                let metrics = metrics.clone();
                move |request, next| instrument_request(request, next, metrics.clone())
            }));

        for path in ["/health", "/ready"] {
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::get(path)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK);
        }
        assert_eq!(
            metrics
                .completed_requests
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            metrics
                .active_balance
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );

        let response = router
            .oneshot(
                axum::http::Request::get("/normal?private=value")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            metrics
                .completed_requests
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert_eq!(
            metrics
                .active_balance
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn emitted_http_signals_exclude_request_and_error_data() {
        const CHILD_ENV: &str = "FAKTORY_TELEMETRY_EMISSION_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            let child =
                std::process::Command::new(std::env::current_exe().expect("test executable"))
                    .args([
                        "--exact",
                        "app::tests::emitted_http_signals_exclude_request_and_error_data",
                    ])
                    .env(CHILD_ENV, "1")
                    .output()
                    .expect("isolated telemetry test");
            assert!(
                child.status.success(),
                "isolated telemetry test failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr)
            );
            return;
        }
        assert_emitted_http_signals();
    }

    fn assert_emitted_http_signals() {
        let span_exporter = InMemorySpanExporter::default();
        let tracer_provider = SdkTracerProvider::builder()
            .with_simple_exporter(span_exporter.clone())
            .build();
        let tracer = tracer_provider.tracer("faktory-test");
        let metric_exporter = InMemoryMetricExporter::default();
        let meter_provider = SdkMeterProvider::builder()
            .with_periodic_exporter(metric_exporter.clone())
            .build();
        let metrics = HttpMetrics::from_meter(meter_provider.meter("faktory.http"));
        let output = RecordingWriter::default();
        let writer = output.clone();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(crate::observability::JsonEventFormatter)
                    .with_writer(move || writer.clone())
                    .with_filter(crate::observability::json_filter()),
            )
            .with(crate::observability::trace_filter());
        let dispatch = tracing::Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatch, || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(async {
                    let router = Router::new()
                        .route(
                            "/models/{model_id}",
                            axum::routing::post(|| async {
                                (StatusCode::SERVICE_UNAVAILABLE, "private-error-response")
                            }),
                        )
                        .layer(middleware::from_fn(move |request, next| {
                            instrument_request(request, next, metrics.clone())
                        }));
                    let response = router
                        .oneshot(
                            Request::post("/models/private-model-id?token=private-query-value")
                                .header(header::AUTHORIZATION, "Bearer private-authorization-token")
                                .body(Body::from("private-source-body"))
                                .expect("request"),
                        )
                        .await
                        .expect("response");
                    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                    assert_eq!(
                        to_bytes(response.into_body(), 1024)
                            .await
                            .expect("response body"),
                        "private-error-response"
                    );
                });
        });
        tracer_provider.force_flush().expect("flush spans");
        meter_provider.force_flush().expect("flush metrics");
        assert_stdout_request_event(&output);
        assert_request_span(&span_exporter);
        assert_request_metrics(&metric_exporter);
    }

    #[test]
    fn dropped_request_is_finalized_as_cancelled() {
        let metrics = HttpMetrics::new();
        let guard = RequestMetricsGuard::new(
            metrics.clone(),
            tracing::info_span!("http.server.request"),
            "POST".to_owned(),
            "/mcp".to_owned(),
        );
        assert_eq!(
            metrics
                .active_balance
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        drop(guard);
        assert_eq!(
            metrics
                .active_balance
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            metrics
                .completed_requests
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn production_transport_gate_accepts_only_grpc_web_content_types() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/grpc"),
        );
        assert!(!is_grpc_web(&headers));
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/grpc-web+proto"),
        );
        assert!(is_grpc_web(&headers));
    }

    fn disabled_config() -> AppConfig {
        AppConfig {
            auth: AuthConfig::Disabled,
            public_base_url: "http://127.0.0.1:3000".to_owned(),
            static_directory: None,
            render: RenderConfig {
                command: vec!["/usr/bin/false".to_owned()],
                queue_capacity: 2,
                concurrency: 1,
                timeout: std::time::Duration::from_secs(1),
                max_output_bytes: 1024,
            },
            watch_capacity: 4,
        }
    }

    async fn artifact_request(router: Router, path: String) -> Response {
        artifact_request_with_header(router, path, None).await
    }

    async fn artifact_request_with_header(
        router: Router,
        path: String,
        header: Option<(header::HeaderName, HeaderValue)>,
    ) -> Response {
        let mut request = axum::http::Request::builder().uri(path);
        if let Some((name, value)) = header {
            request = request.header(name, value);
        }
        router
            .oneshot(request.body(Body::empty()).expect("request"))
            .await
            .expect("router response")
    }

    async fn runtime_with_completed_model() -> (Runtime, crate::model::ModelRecord) {
        let runtime = build_runtime(Arc::new(InMemoryObjectStore::default()), disabled_config())
            .await
            .expect("runtime");
        let model = runtime
            .repository()
            .create_model("part", "Part", b"first")
            .await
            .expect("first source");
        runtime
            .repository()
            .complete_render(
                &model.id,
                &model.desired_source_revision,
                rendered(b"glTF-artifact"),
            )
            .await
            .expect("first render");
        (runtime, model)
    }

    #[tokio::test]
    async fn preview_is_unavailable_before_first_success() {
        let runtime = build_runtime(Arc::new(InMemoryObjectStore::default()), disabled_config())
            .await
            .expect("runtime");
        let model = runtime
            .repository()
            .create_model("part", "Part", b"first")
            .await
            .expect("first source");
        let response = artifact_request(
            runtime.router(),
            format!(
                "/artifacts/{}/{}/preview.svg",
                model.id, model.desired_source_revision
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn disabled_artifact_route_requires_no_session_and_serves_current_revision() {
        let (runtime, first) = runtime_with_completed_model().await;
        let path = format!(
            "/artifacts/{}/{}/model.glb",
            first.id, first.desired_source_revision
        );
        let response = artifact_request(runtime.router(), path).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 1024)
                .await
                .expect("artifact body"),
            bytes::Bytes::from_static(b"glTF-artifact")
        );
        let preview = artifact_request(
            runtime.router(),
            format!(
                "/artifacts/{}/{}/preview.svg",
                first.id, first.desired_source_revision
            ),
        )
        .await;
        assert_eq!(preview.status(), StatusCode::OK);
        assert_eq!(preview.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert_eq!(
            preview.headers()[header::CACHE_CONTROL],
            "private, no-cache"
        );
        assert_eq!(preview.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(
            preview.headers()[header::CONTENT_SECURITY_POLICY],
            "default-src 'none'; style-src 'unsafe-inline'; sandbox"
        );
        assert_eq!(
            preview.headers()[header::ETAG],
            format!("\"{}\"", first.desired_source_revision)
        );
        assert_eq!(preview.headers()[header::CONTENT_LENGTH], "11");
        assert_eq!(
            to_bytes(preview.into_body(), 1024)
                .await
                .expect("preview body"),
            bytes::Bytes::from_static(b"<svg></svg>")
        );
        let wrong = artifact_request(
            runtime.router(),
            format!("/artifacts/{}/{}/model.glb", first.id, "0".repeat(64)),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::NOT_FOUND);
        let wrong_preview = artifact_request(
            runtime.router(),
            format!("/artifacts/{}/{}/preview.svg", first.id, "0".repeat(64)),
        )
        .await;
        assert_eq!(wrong_preview.status(), StatusCode::NOT_FOUND);
        for suffix in ["model.glb", "preview.svg"] {
            let malformed = artifact_request(
                runtime.router(),
                format!("/artifacts/{}/not-a-revision/{suffix}", first.id),
            )
            .await;
            assert_eq!(malformed.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn artifact_conditionals_and_glb_ranges_follow_http_semantics() {
        let (runtime, first) = runtime_with_completed_model().await;
        let glb_path = format!(
            "/artifacts/{}/{}/model.glb",
            first.id, first.desired_source_revision
        );
        let preview_path = format!(
            "/artifacts/{}/{}/preview.svg",
            first.id, first.desired_source_revision
        );
        for path in [glb_path.clone(), preview_path.clone()] {
            for validator in [
                "*".to_owned(),
                format!("W/\"{}\"", first.desired_source_revision),
            ] {
                let response = artifact_request_with_header(
                    runtime.router(),
                    path.clone(),
                    Some((
                        header::IF_NONE_MATCH,
                        HeaderValue::from_str(&validator).expect("validator"),
                    )),
                )
                .await;
                assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
                if path.ends_with("preview.svg") {
                    assert_eq!(response.headers()[header::CONTENT_LENGTH], "0");
                    assert_eq!(
                        response.headers()[header::CONTENT_SECURITY_POLICY],
                        "default-src 'none'; style-src 'unsafe-inline'; sandbox"
                    );
                }
            }
            let unrelated = artifact_request_with_header(
                runtime.router(),
                path,
                Some((
                    header::IF_NONE_MATCH,
                    HeaderValue::from_static("W/\"unrelated\""),
                )),
            )
            .await;
            assert_eq!(unrelated.status(), StatusCode::OK);
        }

        let range = artifact_request_with_header(
            runtime.router(),
            glb_path,
            Some((header::RANGE, HeaderValue::from_static("bytes=2-5"))),
        )
        .await;
        assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(range.headers()[header::CONTENT_TYPE], "model/gltf-binary");
        assert_eq!(range.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(range.headers()[header::CONTENT_RANGE], "bytes 2-5/13");
        assert_eq!(range.headers()[header::CONTENT_LENGTH], "4");
        assert_eq!(
            to_bytes(range.into_body(), 1024).await.expect("range body"),
            bytes::Bytes::from_static(b"TF-a")
        );
    }

    #[tokio::test]
    async fn retained_last_good_is_only_authorized_revision_after_failure() {
        let runtime = build_runtime(Arc::new(InMemoryObjectStore::default()), disabled_config())
            .await
            .expect("runtime");
        let first = runtime
            .repository()
            .create_model("part", "Part", b"first")
            .await
            .expect("first source");
        runtime
            .repository()
            .complete_render(
                &first.id,
                &first.desired_source_revision,
                rendered(b"glTF-old"),
            )
            .await
            .expect("first render");
        let replacement = runtime
            .repository()
            .edit_model(
                &first.id,
                &first.desired_source_revision,
                None,
                Some(&[crate::model::SourcePatch {
                    old: "first".to_owned(),
                    new: "replacement".to_owned(),
                }]),
            )
            .await
            .expect("replacement source")
            .record;
        runtime
            .repository()
            .fail_render(
                &replacement.id,
                &replacement.desired_source_revision,
                "safe failure",
            )
            .await
            .expect("replacement failure");
        let retained = artifact_request(
            runtime.router(),
            format!(
                "/artifacts/{}/{}/model.glb",
                first.id, first.desired_source_revision
            ),
        )
        .await;
        assert_eq!(retained.status(), StatusCode::OK);
        let retained_preview = artifact_request(
            runtime.router(),
            format!(
                "/artifacts/{}/{}/preview.svg",
                first.id, first.desired_source_revision
            ),
        )
        .await;
        assert_eq!(retained_preview.status(), StatusCode::OK);
        let failed = artifact_request(
            runtime.router(),
            format!(
                "/artifacts/{}/{}/model.glb",
                replacement.id, replacement.desired_source_revision
            ),
        )
        .await;
        assert_eq!(failed.status(), StatusCode::NOT_FOUND);
        let failed_preview = artifact_request(
            runtime.router(),
            format!(
                "/artifacts/{}/{}/preview.svg",
                replacement.id, replacement.desired_source_revision
            ),
        )
        .await;
        assert_eq!(failed_preview.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn disabled_spa_grpc_web_and_mcp_routes_require_no_credentials() {
        let directory = tempfile::tempdir().expect("static directory");
        tokio::fs::write(directory.path().join("index.html"), b"Faktory SPA")
            .await
            .expect("write index");
        let mut config = disabled_config();
        config.static_directory = Some(directory.path().to_owned());
        let runtime = build_runtime(Arc::new(InMemoryObjectStore::default()), config)
            .await
            .expect("runtime");

        let spa = runtime
            .router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("SPA request"),
            )
            .await
            .expect("SPA response");
        assert_eq!(spa.status(), StatusCode::OK);

        let direct_spa_link = runtime
            .router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/models/direct-link")
                    .body(Body::empty())
                    .expect("direct SPA request"),
            )
            .await
            .expect("direct SPA response");
        assert_eq!(direct_spa_link.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(direct_spa_link.into_body(), usize::MAX)
                .await
                .expect("read direct SPA response"),
            bytes::Bytes::from_static(b"Faktory SPA")
        );

        let grpc = runtime
            .router()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/faktory.v1.FaktoryService/ListModels")
                    .header(header::CONTENT_TYPE, "application/grpc-web+proto")
                    .header("te", "trailers")
                    .body(Body::from([0_u8, 0, 0, 0, 0].as_slice()))
                    .expect("gRPC-web request"),
            )
            .await
            .expect("gRPC-web response");
        assert_eq!(grpc.status(), StatusCode::OK);
        let grpc_body = to_bytes(grpc.into_body(), usize::MAX)
            .await
            .expect("read gRPC-web response");
        assert_eq!(grpc_body.first(), Some(&0), "missing unary response frame");
        assert!(
            grpc_body
                .windows(b"grpc-status:0".len())
                .any(|window| window == b"grpc-status:0"),
            "gRPC-web response did not report success"
        );

        let mcp = runtime
            .router()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("mcp-protocol-version", "2026-07-28")
                    .body(Body::from("{broken"))
                    .expect("MCP request"),
            )
            .await
            .expect("MCP response");
        assert_eq!(mcp.status(), StatusCode::BAD_REQUEST);
    }
}
