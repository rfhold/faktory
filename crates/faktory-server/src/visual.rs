//! Bounded client and coordinator for the isolated visual renderer.

use std::{
    collections::{HashMap, hash_map::Entry},
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::{Bytes, BytesMut};
use faktory_proto::v1::Projection;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, oneshot};
use url::Url;

use crate::{
    model::{TechnicalProjection, ViewRecord, ViewRenderIdentity},
    render::{PROJECTION_HEIGHT, PROJECTION_WIDTH, validate_visual_png},
};

pub const VISUAL_RECIPE: &str = "three-v2";
pub const MAX_VISUAL_GLB_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_VISUAL_RESPONSE_BYTES: usize = 24 * 1024 * 1024;
const VISUAL_QUEUE_CAPACITY: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VisualRendererError {
    Unavailable,
    InvalidResponse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VisualRenderResult {
    pub images: Vec<(String, Bytes)>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VisualRenderSpec {
    Canonical {
        recipe: &'static str,
    },
    View {
        recipe: &'static str,
        camera: VisualCamera,
    },
}

impl VisualRenderSpec {
    #[must_use]
    pub const fn canonical() -> Self {
        Self::Canonical {
            recipe: VISUAL_RECIPE,
        }
    }

    pub fn from_view(view: &ViewRecord) -> Result<Self, VisualRendererError> {
        let projection = match Projection::try_from(view.projection) {
            Ok(Projection::Perspective) => VisualProjection::Perspective,
            Ok(Projection::Orthographic) => VisualProjection::Orthographic,
            _ => return Err(VisualRendererError::InvalidResponse),
        };
        Ok(Self::View {
            recipe: VISUAL_RECIPE,
            camera: VisualCamera {
                target: view.target,
                rotation: view.rotation,
                projection,
                distance: view.distance,
                field_of_view_degrees: view.field_of_view_degrees,
                orthographic_scale: view.orthographic_scale,
            },
        })
    }

    const fn is_canonical(&self) -> bool {
        matches!(self, Self::Canonical { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VisualCamera {
    pub target: [f64; 3],
    pub rotation: [f64; 4],
    pub projection: VisualProjection,
    pub distance: f64,
    pub field_of_view_degrees: f64,
    pub orthographic_scale: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualProjection {
    Perspective,
    Orthographic,
}

#[async_trait]
pub trait VisualRenderer: Send + Sync + fmt::Debug {
    async fn render(
        &self,
        glb: Bytes,
        spec: VisualRenderSpec,
    ) -> Result<VisualRenderResult, VisualRendererError>;
}

#[derive(Clone)]
pub struct HttpVisualRenderer {
    client: reqwest::Client,
    endpoint: Url,
    timeout: Duration,
}

impl fmt::Debug for HttpVisualRenderer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpVisualRenderer")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl HttpVisualRenderer {
    pub fn new(base_url: &str, timeout: Duration) -> Result<Self, VisualRendererError> {
        let base = Url::parse(base_url).map_err(|_| VisualRendererError::InvalidResponse)?;
        if !matches!(base.scheme(), "http" | "https")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || timeout.is_zero()
        {
            return Err(VisualRendererError::InvalidResponse);
        }
        let endpoint = base
            .join("/v1/render")
            .map_err(|_| VisualRendererError::InvalidResponse)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map_err(|_| VisualRendererError::InvalidResponse)?;
        Ok(Self {
            client,
            endpoint,
            timeout,
        })
    }
}

#[async_trait]
impl VisualRenderer for HttpVisualRenderer {
    async fn render(
        &self,
        glb: Bytes,
        spec: VisualRenderSpec,
    ) -> Result<VisualRenderResult, VisualRendererError> {
        if glb.len() > MAX_VISUAL_GLB_BYTES {
            return Err(VisualRendererError::InvalidResponse);
        }
        let spec_header = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&spec).map_err(|_| VisualRendererError::InvalidResponse)?);
        let response = tokio::time::timeout(
            self.timeout,
            self.client
                .post(self.endpoint.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .header("x-faktory-render-spec", spec_header)
                .body(glb)
                .send(),
        )
        .await
        .map_err(|_| VisualRendererError::Unavailable)?
        .map_err(|_| VisualRendererError::Unavailable)?;
        if response.status().is_redirection() || response.status().is_server_error() {
            return Err(VisualRendererError::Unavailable);
        }
        if !response.status().is_success() {
            return Err(VisualRendererError::InvalidResponse);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_VISUAL_RESPONSE_BYTES as u64)
        {
            return Err(VisualRendererError::InvalidResponse);
        }
        let mut body = BytesMut::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = tokio::time::timeout(self.timeout, stream.next())
            .await
            .map_err(|_| VisualRendererError::Unavailable)?
        {
            let chunk = chunk.map_err(|_| VisualRendererError::Unavailable)?;
            if body.len().saturating_add(chunk.len()) > MAX_VISUAL_RESPONSE_BYTES {
                return Err(VisualRendererError::InvalidResponse);
            }
            body.extend_from_slice(&chunk);
        }
        decode_response(&body, &spec)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerResponse {
    recipe: String,
    width: u32,
    height: u32,
    images: Vec<WorkerImage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerImage {
    name: String,
    mime_type: String,
    data: String,
}

fn decode_response(
    body: &[u8],
    spec: &VisualRenderSpec,
) -> Result<VisualRenderResult, VisualRendererError> {
    let response: WorkerResponse =
        serde_json::from_slice(body).map_err(|_| VisualRendererError::InvalidResponse)?;
    if response.recipe != VISUAL_RECIPE
        || response.width != PROJECTION_WIDTH
        || response.height != PROJECTION_HEIGHT
    {
        return Err(VisualRendererError::InvalidResponse);
    }
    let expected: Vec<&str> = if spec.is_canonical() {
        TechnicalProjection::ALL
            .into_iter()
            .map(TechnicalProjection::as_str)
            .collect()
    } else {
        vec!["view"]
    };
    if response.images.len() != expected.len() {
        return Err(VisualRendererError::InvalidResponse);
    }
    let mut images = Vec::with_capacity(expected.len());
    for (image, expected_name) in response.images.into_iter().zip(expected) {
        if image.name != expected_name || image.mime_type != "image/png" {
            return Err(VisualRendererError::InvalidResponse);
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(image.data)
            .map_err(|_| VisualRendererError::InvalidResponse)?;
        validate_visual_png(&bytes).map_err(|_| VisualRendererError::InvalidResponse)?;
        images.push((image.name, Bytes::from(bytes)));
    }
    Ok(VisualRenderResult { images })
}

type Waiters = Vec<oneshot::Sender<Result<VisualRenderResult, VisualRendererError>>>;

#[derive(Clone)]
pub struct VisualCoordinator {
    renderer: Arc<dyn VisualRenderer>,
    admission: Arc<Semaphore>,
    concurrency: Arc<Semaphore>,
    inflight: Arc<Mutex<HashMap<ViewRenderIdentity, Waiters>>>,
    wait_timeout: Duration,
}

impl fmt::Debug for VisualCoordinator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VisualCoordinator")
            .field("wait_timeout", &self.wait_timeout)
            .finish_non_exhaustive()
    }
}

impl VisualCoordinator {
    #[must_use]
    pub fn new(renderer: Arc<dyn VisualRenderer>, wait_timeout: Duration) -> Self {
        Self {
            renderer,
            admission: Arc::new(Semaphore::new(VISUAL_QUEUE_CAPACITY)),
            concurrency: Arc::new(Semaphore::new(1)),
            inflight: Arc::new(Mutex::new(HashMap::new())),
            wait_timeout,
        }
    }

    pub async fn render_canonical(
        &self,
        glb: Bytes,
    ) -> Result<VisualRenderResult, VisualRendererError> {
        let admission = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| VisualRendererError::Unavailable)?;
        let concurrency =
            tokio::time::timeout(self.wait_timeout, self.concurrency.clone().acquire_owned())
                .await
                .map_err(|_| VisualRendererError::Unavailable)?
                .map_err(|_| VisualRendererError::Unavailable)?;
        let result = tokio::time::timeout(
            self.wait_timeout,
            self.renderer.render(glb, VisualRenderSpec::canonical()),
        )
        .await
        .map_err(|_| VisualRendererError::Unavailable)
        .and_then(|result| result)
        .and_then(|result| validate_result(result, true));
        drop((concurrency, admission));
        result
    }

    pub async fn render_view(
        &self,
        identity: ViewRenderIdentity,
        glb: Bytes,
        spec: VisualRenderSpec,
    ) -> Result<VisualRenderResult, VisualRendererError> {
        let (sender, receiver) = oneshot::channel();
        let joined_existing = {
            let mut inflight = self
                .inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match inflight.entry(identity.clone()) {
                Entry::Occupied(mut entry) => {
                    entry.get_mut().push(sender);
                    true
                }
                Entry::Vacant(entry) => {
                    entry.insert(vec![sender]);
                    false
                }
            }
        };
        if joined_existing {
            return receiver
                .await
                .unwrap_or(Err(VisualRendererError::Unavailable));
        }
        let Ok(admission) = self.admission.clone().try_acquire_owned() else {
            self.finish_view(&identity, Err(VisualRendererError::Unavailable));
            return receiver
                .await
                .unwrap_or(Err(VisualRendererError::Unavailable));
        };
        let coordinator = self.clone();
        tokio::spawn(async move {
            let result = match tokio::time::timeout(
                coordinator.wait_timeout,
                coordinator.concurrency.clone().acquire_owned(),
            )
            .await
            {
                Ok(Ok(concurrency)) => {
                    let result = tokio::time::timeout(
                        coordinator.wait_timeout,
                        coordinator.renderer.render(glb, spec),
                    )
                    .await
                    .map_err(|_| VisualRendererError::Unavailable)
                    .and_then(|result| result)
                    .and_then(|result| validate_result(result, false));
                    drop(concurrency);
                    result
                }
                _ => Err(VisualRendererError::Unavailable),
            };
            drop(admission);
            coordinator.finish_view(&identity, result);
        });
        receiver
            .await
            .unwrap_or(Err(VisualRendererError::Unavailable))
    }

    fn finish_view(
        &self,
        identity: &ViewRenderIdentity,
        result: Result<VisualRenderResult, VisualRendererError>,
    ) {
        let waiters = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(identity)
            .unwrap_or_default();
        for waiter in waiters {
            drop(waiter.send(result.clone()));
        }
    }
}

fn validate_result(
    result: VisualRenderResult,
    canonical: bool,
) -> Result<VisualRenderResult, VisualRendererError> {
    let expected: Vec<&str> = if canonical {
        TechnicalProjection::ALL
            .into_iter()
            .map(TechnicalProjection::as_str)
            .collect()
    } else {
        vec!["view"]
    };
    if result.images.len() != expected.len()
        || result
            .images
            .iter()
            .zip(expected)
            .any(|((name, image), expected)| {
                name != expected || validate_visual_png(image).is_err()
            })
    {
        Err(VisualRendererError::InvalidResponse)
    } else {
        Ok(result)
    }
}

#[derive(Debug)]
pub struct UnavailableVisualRenderer;

#[async_trait]
impl VisualRenderer for UnavailableVisualRenderer {
    async fn render(
        &self,
        _glb: Bytes,
        _spec: VisualRenderSpec,
    ) -> Result<VisualRenderResult, VisualRendererError> {
        Err(VisualRendererError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::Response,
        routing::post,
    };
    use tokio::{sync::Semaphore as TokioSemaphore, task::JoinHandle};

    use super::*;

    fn valid_png() -> Bytes {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(PROJECTION_WIDTH, PROJECTION_HEIGHT)
            .expect("visual pixmap");
        pixmap.fill(resvg::tiny_skia::Color::WHITE);
        Bytes::from(pixmap.encode_png().expect("visual PNG"))
    }

    fn response_json(names: &[&str]) -> String {
        let data = base64::engine::general_purpose::STANDARD.encode(valid_png());
        serde_json::json!({
            "recipe": VISUAL_RECIPE,
            "width": PROJECTION_WIDTH,
            "height": PROJECTION_HEIGHT,
            "images": names.iter().map(|name| serde_json::json!({
                "name": name,
                "mime_type": "image/png",
                "data": data
            })).collect::<Vec<_>>()
        })
        .to_string()
    }

    #[derive(Clone)]
    struct HttpState {
        captured: Arc<Mutex<Vec<(HeaderMap, Bytes)>>>,
        status: StatusCode,
        body: String,
        delay: Duration,
        content_length: Option<u64>,
    }

    struct TestServer {
        base_url: String,
        captured: Arc<Mutex<Vec<(HeaderMap, Bytes)>>>,
        task: JoinHandle<()>,
    }

    impl TestServer {
        async fn start(status: StatusCode, body: String, delay: Duration) -> Self {
            Self::start_with_length(status, body, delay, None).await
        }

        async fn start_with_length(
            status: StatusCode,
            body: String,
            delay: Duration,
            content_length: Option<u64>,
        ) -> Self {
            let captured = Arc::new(Mutex::new(Vec::new()));
            let state = HttpState {
                captured: captured.clone(),
                status,
                body,
                delay,
                content_length,
            };
            let router = Router::new()
                .route("/v1/render", post(handler))
                .with_state(state);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind visual test server");
            let address = listener.local_addr().expect("visual test address");
            let task = tokio::spawn(async move {
                axum::serve(listener, router)
                    .await
                    .expect("serve visual test");
            });
            Self {
                base_url: format!("http://{address}"),
                captured,
                task,
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn handler(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
        state
            .captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((headers, body));
        tokio::time::sleep(state.delay).await;
        let mut response = Response::builder().status(state.status);
        if let Some(length) = state.content_length {
            response = response.header(reqwest::header::CONTENT_LENGTH, length);
        }
        response
            .body(Body::from(state.body))
            .expect("visual test response")
    }

    #[tokio::test]
    async fn http_rpc_sends_exact_canonical_contract_and_accepts_valid_response() {
        let names = [
            "isometric",
            "front",
            "back",
            "left",
            "right",
            "top",
            "bottom",
        ];
        let server = TestServer::start(StatusCode::OK, response_json(&names), Duration::ZERO).await;
        let renderer = HttpVisualRenderer::new(&server.base_url, Duration::from_secs(1))
            .expect("HTTP visual renderer");

        let result = renderer
            .render(
                Bytes::from_static(b"exact-glb"),
                VisualRenderSpec::canonical(),
            )
            .await
            .expect("visual response");

        assert_eq!(result.images.len(), 7);
        let captured = server
            .captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].1, Bytes::from_static(b"exact-glb"));
        assert_eq!(
            captured[0].0[reqwest::header::CONTENT_TYPE],
            "application/octet-stream"
        );
        let encoded = captured[0].0["x-faktory-render-spec"]
            .to_str()
            .expect("spec header");
        assert_eq!(
            URL_SAFE_NO_PAD.decode(encoded).expect("decode spec"),
            br#"{"kind":"canonical","recipe":"three-v2"}"#
        );
        assert!(!captured[0].0.contains_key(reqwest::header::AUTHORIZATION));
        drop(captured);
    }

    #[tokio::test]
    async fn http_rpc_bounds_failures_without_exposing_response_data() {
        let oversized = TestServer::start_with_length(
            StatusCode::OK,
            String::new(),
            Duration::ZERO,
            Some((MAX_VISUAL_RESPONSE_BYTES + 1) as u64),
        )
        .await;
        let renderer =
            HttpVisualRenderer::new(&oversized.base_url, Duration::from_secs(1)).expect("renderer");
        assert_eq!(
            renderer
                .render(Bytes::new(), VisualRenderSpec::canonical())
                .await,
            Err(VisualRendererError::InvalidResponse)
        );

        let unavailable = TestServer::start(
            StatusCode::INTERNAL_SERVER_ERROR,
            "private-browser-stderr".to_owned(),
            Duration::ZERO,
        )
        .await;
        let renderer = HttpVisualRenderer::new(&unavailable.base_url, Duration::from_secs(1))
            .expect("renderer");
        let error = renderer
            .render(Bytes::new(), VisualRenderSpec::canonical())
            .await
            .expect_err("5xx unavailable");
        assert_eq!(error, VisualRendererError::Unavailable);
        assert!(!format!("{error:?}").contains("private-browser-stderr"));

        let redirect = TestServer::start(
            StatusCode::TEMPORARY_REDIRECT,
            String::new(),
            Duration::ZERO,
        )
        .await;
        let renderer =
            HttpVisualRenderer::new(&redirect.base_url, Duration::from_secs(1)).expect("renderer");
        assert_eq!(
            renderer
                .render(Bytes::new(), VisualRenderSpec::canonical())
                .await,
            Err(VisualRendererError::Unavailable)
        );
        assert_eq!(
            redirect
                .captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1
        );

        let slow = TestServer::start(
            StatusCode::OK,
            response_json(&["view"]),
            Duration::from_millis(100),
        )
        .await;
        let renderer =
            HttpVisualRenderer::new(&slow.base_url, Duration::from_millis(10)).expect("renderer");
        assert_eq!(
            renderer
                .render(
                    Bytes::new(),
                    VisualRenderSpec::View {
                        recipe: VISUAL_RECIPE,
                        camera: VisualCamera {
                            target: [0.0; 3],
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            projection: VisualProjection::Perspective,
                            distance: 1.0,
                            field_of_view_degrees: 45.0,
                            orthographic_scale: 1.0,
                        },
                    },
                )
                .await,
            Err(VisualRendererError::Unavailable)
        );
    }

    #[test]
    fn endpoint_validation_and_debug_hide_endpoint() {
        for invalid in [
            "ftp://worker.internal",
            "http://user:pass@worker.internal",
            "http://worker.internal?secret=value",
            "http://worker.internal#fragment",
        ] {
            assert!(HttpVisualRenderer::new(invalid, Duration::from_secs(1)).is_err());
        }
        let renderer = HttpVisualRenderer::new(
            "http://private-worker.internal:9876",
            Duration::from_secs(1),
        )
        .expect("renderer");
        assert!(!format!("{renderer:?}").contains("private-worker"));
        let oversized_glb = vec![0_u8; MAX_VISUAL_GLB_BYTES + 1];
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        assert_eq!(
            runtime.block_on(
                renderer.render(Bytes::from(oversized_glb), VisualRenderSpec::canonical(),)
            ),
            Err(VisualRendererError::InvalidResponse)
        );
    }

    #[test]
    fn response_protocol_is_strict_about_metadata_order_and_opacity() {
        let mut wrong_width: serde_json::Value =
            serde_json::from_str(&response_json(&["view"])).expect("valid response fixture");
        wrong_width["width"] = serde_json::json!(639);
        assert_eq!(
            decode_response(
                wrong_width.to_string().as_bytes(),
                &VisualRenderSpec::View {
                    recipe: VISUAL_RECIPE,
                    camera: VisualCamera {
                        target: [0.0; 3],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        projection: VisualProjection::Perspective,
                        distance: 1.0,
                        field_of_view_degrees: 45.0,
                        orthographic_scale: 1.0,
                    },
                },
            ),
            Err(VisualRendererError::InvalidResponse)
        );

        let wrong_order = response_json(&[
            "front",
            "isometric",
            "back",
            "left",
            "right",
            "top",
            "bottom",
        ]);
        assert_eq!(
            decode_response(wrong_order.as_bytes(), &VisualRenderSpec::canonical()),
            Err(VisualRendererError::InvalidResponse)
        );

        let transparent = resvg::tiny_skia::Pixmap::new(PROJECTION_WIDTH, PROJECTION_HEIGHT)
            .expect("transparent pixmap")
            .encode_png()
            .expect("transparent PNG");
        let transparent = serde_json::json!({
            "recipe": VISUAL_RECIPE,
            "width": PROJECTION_WIDTH,
            "height": PROJECTION_HEIGHT,
            "images": [{
                "name":"view",
                "mime_type":"image/png",
                "data":base64::engine::general_purpose::STANDARD.encode(transparent)
            }]
        });
        assert_eq!(
            decode_response(
                transparent.to_string().as_bytes(),
                &VisualRenderSpec::View {
                    recipe: VISUAL_RECIPE,
                    camera: VisualCamera {
                        target: [0.0; 3],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        projection: VisualProjection::Perspective,
                        distance: 1.0,
                        field_of_view_degrees: 45.0,
                        orthographic_scale: 1.0,
                    },
                },
            ),
            Err(VisualRendererError::InvalidResponse)
        );
    }

    #[derive(Debug)]
    struct BlockingRenderer {
        calls: AtomicUsize,
        started: TokioSemaphore,
        release: TokioSemaphore,
    }

    #[async_trait]
    impl VisualRenderer for BlockingRenderer {
        async fn render(
            &self,
            _glb: Bytes,
            spec: VisualRenderSpec,
        ) -> Result<VisualRenderResult, VisualRendererError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.add_permits(1);
            self.release.acquire().await.expect("release open").forget();
            let names: Vec<&str> = if spec.is_canonical() {
                TechnicalProjection::ALL
                    .into_iter()
                    .map(TechnicalProjection::as_str)
                    .collect()
            } else {
                vec!["view"]
            };
            let image = valid_png();
            Ok(VisualRenderResult {
                images: names
                    .into_iter()
                    .map(|name| (name.to_owned(), image.clone()))
                    .collect(),
            })
        }
    }

    #[tokio::test]
    async fn view_misses_deduplicate_and_survive_first_caller_cancellation() {
        let renderer = Arc::new(BlockingRenderer {
            calls: AtomicUsize::new(0),
            started: TokioSemaphore::new(0),
            release: TokioSemaphore::new(0),
        });
        let coordinator = VisualCoordinator::new(renderer.clone(), Duration::from_secs(1));
        let identity = ViewRenderIdentity {
            revision: "a".repeat(64),
            output_id: "primary".to_owned(),
            view_id: "view-id".to_owned(),
            view_etag: "view-etag".to_owned(),
        };
        let spec = VisualRenderSpec::View {
            recipe: VISUAL_RECIPE,
            camera: VisualCamera {
                target: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                projection: VisualProjection::Orthographic,
                distance: 2.0,
                field_of_view_degrees: 45.0,
                orthographic_scale: 3.0,
            },
        };
        let first_coordinator = coordinator.clone();
        let first_identity = identity.clone();
        let first_spec = spec.clone();
        let first = tokio::spawn(async move {
            first_coordinator
                .render_view(first_identity, Bytes::new(), first_spec)
                .await
        });
        renderer.started.acquire().await.expect("started").forget();
        first.abort();
        let second_coordinator = coordinator.clone();
        let second = tokio::spawn(async move {
            second_coordinator
                .render_view(identity, Bytes::new(), spec)
                .await
        });
        tokio::task::yield_now().await;
        renderer.release.add_permits(1);
        assert!(second.await.expect("second task").is_ok());
        assert_eq!(renderer.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn coordinator_rejects_admission_beyond_eight() {
        let renderer = Arc::new(BlockingRenderer {
            calls: AtomicUsize::new(0),
            started: TokioSemaphore::new(0),
            release: TokioSemaphore::new(0),
        });
        let coordinator = VisualCoordinator::new(renderer.clone(), Duration::from_secs(5));
        let spec = VisualRenderSpec::View {
            recipe: VISUAL_RECIPE,
            camera: VisualCamera {
                target: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                projection: VisualProjection::Perspective,
                distance: 1.0,
                field_of_view_degrees: 45.0,
                orthographic_scale: 1.0,
            },
        };
        let mut tasks = Vec::new();
        for index in 0..VISUAL_QUEUE_CAPACITY {
            let coordinator = coordinator.clone();
            let spec = spec.clone();
            tasks.push(tokio::spawn(async move {
                coordinator
                    .render_view(
                        ViewRenderIdentity {
                            revision: "a".repeat(64),
                            output_id: "primary".to_owned(),
                            view_id: format!("view-{index}"),
                            view_etag: format!("etag-{index}"),
                        },
                        Bytes::new(),
                        spec,
                    )
                    .await
            }));
        }
        renderer
            .started
            .acquire()
            .await
            .expect("first started")
            .forget();
        while coordinator
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
            < VISUAL_QUEUE_CAPACITY
        {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            coordinator
                .render_view(
                    ViewRenderIdentity {
                        revision: "a".repeat(64),
                        output_id: "primary".to_owned(),
                        view_id: "overflow".to_owned(),
                        view_etag: "overflow-etag".to_owned(),
                    },
                    Bytes::new(),
                    spec,
                )
                .await,
            Err(VisualRendererError::Unavailable)
        );
        renderer.release.add_permits(VISUAL_QUEUE_CAPACITY);
        for task in tasks {
            assert!(task.await.expect("render task").is_ok());
        }
    }
}
