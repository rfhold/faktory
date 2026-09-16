# Protocol

## Authority

[`proto/faktory/v1/faktory.proto`](../../proto/faktory/v1/faktory.proto) is authoritative for the versioned wire schema. Committed Rust bindings live in [`crates/faktory-proto`](../../crates/faktory-proto/). Buf generates browser TypeScript descriptors under `proto/gen/ts`; the Solid app uses them with `@connectrpc/connect-web`.

## Service

`faktory.v1.FaktoryService` provides unary `ListModels`, `GetModel`, `ListViews`, `PutView`, `DeleteView`, and `SetDefaultView`, plus server-streaming `WatchModels`. Production requires an authenticated browser session at the gRPC-web boundary. Explicit disabled mode admits the same calls without credentials. Compose exposes that mode at `http://172.16.1.40:8080` on a trusted private network; its public-base-derived absolute URLs use the same origin.

MCP tools remain outside the protobuf service contract. Bulk `model.create`, `model.get`, and `model.edit` use the hard-cutover project schemas in [`model-projects-libraries.md`](model-projects-libraries.md). Agent-oriented `model.open`, `model.read`, `model.glob`, `model.grep`, and `model.apply_patch` operate over those same canonical revisions. `library.open` and `library.read` provide bounded access to the immutable releases also served by bulk `library.get`. All tool input objects reject unknown fields, and singular `source` inputs and outputs remain absent. `model.inspect` accepts `model_id`, one closed `projection` value, and optional `render_style`. The style accepts `technical` or `shaded` and defaults to `technical`. The tool returns text, exactly one base64 `image/png` block, structured metadata without image bytes, and `isError: false`. Metadata includes identity, 640x480 dimensions, desired and rendered revisions, desired-revision render state, MIME type, and `stale`. Shaded metadata also includes `style` and recipe `three-v2`. `stale` equals true when desired and rendered revisions differ. A stale result returns the retained last-good image with explicit warning text.

`view.inspect` accepts exactly `model_id` and `view_id`. It returns one shaded 640x480 PNG for the exact saved camera. Its metadata includes model and view identity, view etag, style, recipe, dimensions, revisions, state, MIME type, and `stale`. The tool rechecks the successful revision and view etag before storage and before return. A concurrent source success or view mutation returns a safe conflict instead of mismatched bytes.

A model without a successful render returns a safe not-found error. A successful revision with only legacy `three-v1` canonical objects also returns not found because the active `three-v2` recipe never selects those immutable objects. Corrupt stored images return invalid state. Worker absence, timeout, crash, or saturation returns a retry-enabled unavailable error for on-demand work. Invalid input returns invalid argument. Non-retryable identity races return conflict. [`storage-rendering.md`](storage-rendering.md) defines model IDs, project changes, legacy behavior, and render caches. Protobuf model records remain metadata-only, and the project cutover adds no protobuf fields.

`Model.desired_source_revision` retains its protobuf name but contains the lowercase canonical project SHA-256 selected by the latest accepted project creation, project edit, or compatible library rollout. `current_successful_source_revision` identifies the project revision whose GLB, preview, and facts remain available. They differ during a render and after a failed replacement, except that an explicit same-project rerender keeps them equal while work is pending. `render_state` describes work for the desired revision; `render_error` is empty except for a safe, bounded failure summary.

`Model.current_successful_facts` contains volume in cubic millimetres and source-coordinate axis-aligned x/y/z dimensions in millimetres. The field is absent when no successful geometry exists. `Model.updated_at` records the latest accepted model create, project edit, compatible-library rollout, or name edit. Render-state transitions and view mutations do not change it. List, get, snapshot, and change responses carry these fields in complete `Model` records.

## Watch Ordering

Every successful `WatchModels` stream sends exactly one `initial_snapshot` as its first message. That snapshot is authoritative at the stream's synchronization point and replaces the client's model collection. Every later message is `model_changed` and upserts the complete included model record. Model deletion is absent from the MVP, so no deletion event exists.

Clients must reject a stream whose first event is not `initial_snapshot`. Reconnection starts a new stream and replaces local state from its new snapshot; clients must not merge an old connection's pending events into the new snapshot.

## Named Views

A named view stores an ID, display name, target XYZ, rotation quaternion XYZW, projection, camera distance, vertical field of view in degrees, orthographic scale, and opaque etag. Rotation quaternions must be finite and non-zero and are normalized by the server. Numeric camera fields must be finite and positive where applicable. Perspective views use distance and field of view; orthographic views use distance and orthographic scale.

For orthographic views, `orthographic_scale` means the effective visible vertical span. The browser computes it as `(top - bottom) / zoom`. Applying a stored orthographic view resets camera zoom to `1`. Resize preserves the effective span. A perspective-to-orthographic toggle preserves the perspective footprint `2 * distance * tan(vertical_fov / 2)`. The reverse toggle derives distance from the same footprint. This bidirectional conversion prevents scale drift. A view saved by an older client can contain a scale that lost zoom information; the current client cannot reconstruct that old zoom.

`PutView` creates a view when `view.id` and `expected_etag` are absent. It updates a current view when both are present and the etag matches. The server assigns opaque UUID view IDs and replacement etags; clients must not interpret them. `DeleteView` requires the current etag. A mismatch returns gRPC `ABORTED`; missing models or views return `NOT_FOUND`; invalid fields return `INVALID_ARGUMENT`.

`SetDefaultView` selects an existing view for the model. Deleting the default view clears `default_view_id`. All authenticated web users share the same views and may mutate them.

## Artifact HTTP

Project files, library source, guidance, docs, geometry, preview, and projection bytes never travel in protobuf messages. Source text travels only through authorized MCP project and library tools. Metadata facts and `updated_at` travel in complete `Model` records. Projection PNGs travel only in semantic MCP image results, never through browser artifact URLs.

The server exposes `GET /artifacts/{model_id}/{revision}/model.glb` and `GET /artifacts/{model_id}/{revision}/preview.svg`. Production requires a browser session. Explicit disabled mode requires no credentials. Each route serves an artifact only when `revision` matches the model's current successful revision. This gate includes the retained last-good revision after a failed replacement. A model without a successful render returns `404`. A request for any other revision also returns `404`.

Successful responses use `model/gltf-binary` or `image/svg+xml` as appropriate. They use `Cache-Control: private, no-cache` and an entity tag derived from the project revision. Both routes support conditional requests. GLB responses also support single-range requests.

## Internal Visual Renderer RPC

The server calls `POST /v1/render` on the private visual-renderer service. The request body uses `application/octet-stream` and contains one GLB of at most 64 MiB. Exactly one `X-Faktory-Render-Spec` header carries strict JSON as URL-safe, unpadded base64. The header has a 16 KiB limit. A canonical spec names `kind: canonical` and recipe `three-v2`. A view spec supplies `kind: view`, the recipe, and one complete bounded finite camera.

The response contains either all seven ordered canonical images or one image named `view`. The server and worker enforce recipe identity, names, order, `image/png`, opaque 640x480 pixels, 2 MiB per image, and 24 MiB total response size. The worker permits one active render and a queue of eight. Saturation returns `503`.

The worker starts a fixed 25-second deadline before queue admission and body consumption. The deadline covers queue wait, GLB receipt and validation, browser setup, model load, every frame, PNG validation, and response construction. Expiry removes pending work or requests closure of active browser context work, disposes private harness data, releases queue capacity, and returns `504` with `render_timeout` while the client connection remains available. Context cleanup has a bounded one-second grace period after cancellation. Failure to close within that grace marks the browser unavailable, starts browser shutdown, and makes readiness fail. Client disconnect follows the same cancellation and cleanup path without a second response. Browser failure preserves the persistent process when it remains connected; a disconnected browser makes readiness fail. The internal request carries no authorization header, follows no redirects, and relies on the network boundary in [`access-authentication.md`](access-authentication.md).
