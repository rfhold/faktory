# Protocol

## Authority

[`proto/faktory/v1/faktory.proto`](../../proto/faktory/v1/faktory.proto) is authoritative for the versioned wire schema. Committed Rust bindings live in [`crates/faktory-proto`](../../crates/faktory-proto/). Buf generates browser TypeScript descriptors under `proto/gen/ts`; the Solid app uses them with `@connectrpc/connect-web`.

## Service

`faktory.v1.FaktoryService` provides unary `ListModels`, `GetModel`, `ListViews`, `PutView`, `DeleteView`, and `SetDefaultView`, plus server-streaming `WatchModels`. Production requires an authenticated browser session at the gRPC-web boundary. Explicit disabled mode admits the same calls without credentials for loopback-only Compose use.

MCP model mutation tools are outside the protobuf service contract. [`storage-rendering.md`](storage-rendering.md) defines caller-supplied model IDs and the `model.create` and `model.edit` source contract. Protobuf model ID fields remain strings.

`Model.desired_source_revision` is the lowercase SHA-256 selected by the latest accepted source creation or edit. `current_successful_source_revision` identifies the revision whose GLB, preview, and facts remain available. They differ during a render and after a failed replacement, except that an explicit same-source rerender keeps them equal while work is pending. `render_state` describes work for the desired revision; `render_error` is empty except for a safe, bounded failure summary.

`Model.current_successful_facts` contains volume in cubic millimetres and source-coordinate axis-aligned x/y/z dimensions in millimetres. The field is absent when no successful geometry exists. `Model.updated_at` records the latest accepted model create, source edit, or name edit. Render-state transitions and view mutations do not change it. List, get, snapshot, and change responses carry these fields in complete `Model` records.

## Watch Ordering

Every successful `WatchModels` stream sends exactly one `initial_snapshot` as its first message. That snapshot is authoritative at the stream's synchronization point and replaces the client's model collection. Every later message is `model_changed` and upserts the complete included model record. Model deletion is absent from the MVP, so no deletion event exists.

Clients must reject a stream whose first event is not `initial_snapshot`. Reconnection starts a new stream and replaces local state from its new snapshot; clients must not merge an old connection's pending events into the new snapshot.

## Named Views

A named view stores an ID, display name, target XYZ, rotation quaternion XYZW, projection, camera distance, vertical field of view in degrees, orthographic scale, and opaque etag. Rotation quaternions must be finite and non-zero and are normalized by the server. Numeric camera fields must be finite and positive where applicable. Perspective views use distance and field of view; orthographic views use distance and orthographic scale.

`PutView` creates a view when `view.id` and `expected_etag` are absent. It updates a current view when both are present and the etag matches. The server assigns opaque UUID view IDs and replacement etags; clients must not interpret them. `DeleteView` requires the current etag. A mismatch returns gRPC `ABORTED`; missing models or views return `NOT_FOUND`; invalid fields return `INVALID_ARGUMENT`.

`SetDefaultView` selects an existing view for the model. Deleting the default view clears `default_view_id`. All authenticated web users share the same views and may mutate them.

## Artifact HTTP

Geometry bytes and preview bytes never travel in protobuf messages. Metadata facts and `updated_at` travel in complete `Model` records.

The server exposes `GET /artifacts/{model_id}/{revision}/model.glb` and `GET /artifacts/{model_id}/{revision}/preview.svg`. Production requires a browser session. Explicit disabled mode requires no credentials. Each route serves an artifact only when `revision` matches the model's current successful revision. This gate includes the retained last-good revision after a failed replacement. A model without a successful render returns `404`. A request for any other revision also returns `404`.

Successful responses use `model/gltf-binary` or `image/svg+xml` as appropriate. They use `Cache-Control: private, no-cache` and an entity tag derived from the source revision. Both routes support conditional requests. GLB responses also support single-range requests.
