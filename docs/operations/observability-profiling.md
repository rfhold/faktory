# Observability and Profiling Operations

## Scope

This runbook operates the server contracts in [`../architecture/observability.md`](../architecture/observability.md). Commands and expected messages describe runtime behavior; they are not claims that a workload or telemetry backend is live.

## Configuration

| Configuration | Requirement |
| --- | --- |
| `FAKTORY_DEPLOYMENT_ENVIRONMENT` | Required non-empty resource identity, such as `local`, `preview`, or `prod`. |
| `FAKTORY_K8S_NAMESPACE` | Optional Kubernetes namespace resource attribute and Pyroscope tag. |
| `FAKTORY_K8S_POD_NAME` | Optional pod resource attribute. |
| `FAKTORY_K8S_POD_UID` | Optional pod and service-instance resource attribute. |
| `RUST_LOG` | Optional stdout filter within the fixed target and level allowlist; default `faktory_server=info`. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Optional shared traces and metrics endpoint. Pulumi declares `https://telemetry.holdenitdown.net:4318`. |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` and `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | Optional pair when separate endpoints are needed; both must be set if the shared endpoint is absent. |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | Use `http/protobuf`; Pulumi declares this value. |
| `FAKTORY_PYROSCOPE_URL` | Optional credential-free HTTPS root. Pulumi declares `https://telemetry.holdenitdown.net:4040`. |

Do not put credentials in endpoint URLs or `OTEL_RESOURCE_ATTRIBUTES`. Pulumi supplies stack identity directly and Kubernetes identity through the downward API. Local operation needs only `FAKTORY_DEPLOYMENT_ENVIRONMENT=local` for stdout-only telemetry; omit all OTLP and Pyroscope endpoints to keep external export disabled.

## Startup and Shutdown

On successful startup, stdout contains an `observability initialized` JSON event with service identity and `otlp.enabled`, followed by a bounded Pyroscope enabled or disabled event. Treat missing required environment, `invalid OTLP endpoint configuration`, invalid Pyroscope origin, and exporter or profiler initialization errors as startup configuration failures. Do not retry with credentials embedded in a URL.

On SIGTERM or Ctrl-C, the server drains Axum first, then shuts down Pyroscope and OTLP providers. Allow the declared Kubernetes 30-second termination grace period. Expected completion messages are `service shutdown complete` and `observability shutdown complete`; Pyroscope reports a controlled `completed`, `failed`, or `timed_out` outcome. Absence of a completion event can indicate forced termination, stdout loss, or expiration of the process grace period, but requires runtime evidence to distinguish them.

## Validation Procedure

1. Confirm the intended environment and whether external export is authorized.
2. Inspect process configuration without displaying Secret values. Verify that a shared OTLP endpoint or both signal endpoints are present, never a partial pair.
3. Confirm startup from bounded stdout events. Do not use an endpoint declaration as proof of collector ingestion.
4. Send a non-probe request to a controlled route and confirm only bounded method, route, outcome, status, and duration fields in stdout. `/health` and `/ready` should not produce request telemetry.
5. If target access is separately authorized, validate trace, metric, or profile ingestion in that target and retain the result as execution evidence, not as a canonical documentation claim.
6. Exercise graceful termination only in an authorized disposable or maintenance context; verify bounded shutdown outcomes.

## Troubleshooting

| Symptom | Check and response |
| --- | --- |
| Server exits before binding | Supply non-empty `FAKTORY_DEPLOYMENT_ENVIRONMENT`; check endpoint pairing and the credential-free HTTPS Pyroscope root. |
| Stdout exists but OTLP does not | Confirm `otlp.enabled=true`, `http/protobuf`, destination-unrestricted TCP 4318 egress, DNS/TLS trust, and collector reachability through authorized evidence. The port-based egress declaration is broader than a destination-scoped policy, and neither it nor a configured endpoint proves collector availability. |
| One OTLP signal is missing | A signal-specific configuration requires both trace and metric endpoint variables. Prefer the shared endpoint when both use one origin. |
| Expected debug or dependency event is absent | The fixed contract exports only Faktory server `INFO`, `WARN`, and `ERROR`; `RUST_LOG` cannot widen it. |
| Probe traffic is absent | This is expected. `/health` and `/ready` bypass HTTP telemetry. |
| High-cardinality route values appear | Treat this as a contract regression. Capture a safe route class, stop relying on the affected signal, and add a focused regression test without recording sensitive values. |
| Profiling overhead is unacceptable | Remove `FAKTORY_PYROSCOPE_URL` in a reviewed declaration and redeploy through the authorized delivery path. Profiling is optional and Rust-only. |
| Profiler shutdown times out | The process continues shutdown after the 10-second bound. Investigate network and backend behavior without exposing backend payloads or credentials. |
