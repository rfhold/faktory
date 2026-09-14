# Observability and Browser Telemetry

## Status and Authority

Server telemetry is implemented in [`crates/faktory-server/src/observability.rs`](../../crates/faktory-server/src/observability.rs), HTTP instrumentation in `app.rs`, profiling in `profiling.rs`, and lifecycle ordering in `main.rs`. Browser telemetry is implemented in [`web/src/telemetry.ts`](../../web/src/telemetry.ts), with build gating in `build.ts` and `vite.config.ts` and startup, route, and watch integration in `index.tsx`, `App.tsx`, and `api/watch.ts`. Pulumi declares destinations and identity inputs. These sources define emitted behavior; they do not prove that a collector or deployed workload exists.

## Server Signals

The server always emits newline-delimited JSON events to stdout. It optionally exports OTLP/HTTP protobuf traces and metrics when `OTEL_EXPORTER_OTLP_ENDPOINT` is set, or when both `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` and `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` are set. Setting only one signal-specific endpoint is invalid and fails startup. With no endpoint, stdout JSON remains active and OTLP export is disabled. Pulumi declares the shared endpoint `https://telemetry.holdenitdown.net:4318`; this is a target declaration, not collector availability evidence.

The global W3C Trace Context propagator extracts an inbound parent from HTTP headers. Exported traces use always-on sampling. Trace and metric resources use fixed `service.name=faktory` and `service.namespace=faktory`, required `deployment.environment.name`, and optional `k8s.namespace.name`, `k8s.pod.name`, and `k8s.pod.uid`; pod UID also supplies `service.instance.id`. Correlated stdout events include `trace_id` and `span_id` when a valid current span exists.

Only `INFO`, `WARN`, and `ERROR` events whose target is `faktory_server` or starts with `faktory_server::` may enter stdout or trace export. `RUST_LOG` further filters stdout and defaults to `faktory_server=info`; it does not widen the target or level allowlist. Dependency targets, debug events, and trace-level events are excluded.

## HTTP Contract

Every request except `/health` and `/ready` receives an `http.server.request` span and the following instruments:

| Instrument | Attributes |
| --- | --- |
| `http.server.active_requests` | Bounded method and route. |
| `http.server.request.count` | Bounded method and route, outcome, and status when available. |
| `http.server.request.duration` | The completed-request attributes above, measured in seconds. |

Methods are limited to standard HTTP method names or `OTHER`. Matched Axum route templates are used where available. Unmatched paths collapse to fixed classes for MCP, well-known, authentication, OAuth, OIDC, and Faktory service paths, or to `unmatched`; raw URLs, queries, IDs, and arbitrary paths are not attributes. A response below 500 records `success`, a 5xx response records `error`, and a dropped future records `cancelled` without a status. The request guard balances the active counter and records cancellation during unwinding or early drop. Probe exclusion prevents health traffic from affecting spans, metrics, or completion logs.

## Safety and Lifecycle

Server signals must exclude credentials, tokens, session and authorization material, source text, model or view identifiers, request and response bodies, GLB or SVG bytes, object keys and credentials, raw renderer output, arbitrary errors, query strings, and unbounded paths. Events use controlled messages and fields; repository failures expose only the fixed `repository.error.kind` values `invalid`, `not_found`, `conflict`, `unavailable`, or `corrupt`. Errors exposed by rendering remain safe and bounded as defined in [`access-authentication.md`](access-authentication.md).

Telemetry configuration and initialization occur before authentication, storage, rendering, or listener startup. Invalid required identity, endpoint combinations, exporter initialization, or profiler initialization fail startup. Graceful shutdown stops the HTTP server, gives Pyroscope cleanup up to 10 seconds, then gives each OTLP provider up to 5 seconds to flush and stop. Failures and timeouts produce bounded status events and do not expose backend responses.

## Pyroscope Profiling

CPU profiling is optional and disabled when `FAKTORY_PYROSCOPE_URL` is absent. When present, the value must be a credential-free HTTPS root with no path, query, or fragment. The Rust process uses the `pyroscope-rs` pprof backend at 100 Hz with application name `faktory`, package version, `service_namespace=faktory`, deployment environment, and optional namespace tags. Pod identity is deliberately excluded. Pulumi declares `https://telemetry.holdenitdown.net:4040`.

Profiling covers only the Rust server process. It does not profile the renderer's Python subprocess or the browser. A 100 Hz profiler adds continuous sampling and upload overhead; operators may disable it by omitting the URL when that cost is unsuitable. Shutdown is bounded as described above, and a timed-out cleanup is detached rather than blocking process exit indefinitely.

## Faro Contract and Privacy Exception

The SPA uses `@grafana/faro-web-sdk` and `@grafana/faro-web-tracing` version `2.11.0`. Telemetry is compiled in only for Vite `build` with mode `production`. Preview images use that production build, so preview sends telemetry and reports `app.environment=production`; Faro cannot distinguish preview from production through that field.

Faro uses the fixed endpoint `https://faro.holdenitdown.net/collect`, application name `faktory-spa`, bounded `web/package.json` version, and environment `production`. It initializes before Solid mounts, makes one initialization attempt, and fails open: setup or reporting failures do not block routing, watching, or rendering. Standard web instrumentations, tracing, navigation tracking, and session instrumentation remain enabled. There is no global `beforeSend` sanitizer, URL ignore list, or session-replay integration.

Application-owned page and view values are normalized explicitly to `/`, `/models/:id`, or `/unknown`; query strings, fragments, and model IDs are not retained in those values. Custom `faktory.application_error` events contain only the controlled category `startup`, `dispatch`, or `recovery` and a lowercase bounded outcome, with invalid outcomes replaced by `unknown`.

This is an approved privacy exception to the server signal policy. Kuri-default Faro instrumentation may collect URLs, console output, uncaught exceptions, browser and resource metadata, and session metadata before Faktory's explicit view normalization applies. Faktory does not claim that these standard payloads are sanitized. Sensitive data must not be written to browser URLs, console output, exception messages, or resource names. Session instrumentation is metadata collection, not session replay.
