# Companion Specification

## Authority

This contract defines optional model companions and kit version `1`.
[Model Companions](../companion.md) provides author guidance.
The compiler lives in [`companion.rs`](../../../crates/faktory-server/src/companion.rs).
The trusted kit lives in [`packages/companion-kit/`](../../../packages/companion-kit/).
The isolated host lives in [`apps/companion-host/`](../../../apps/companion-host/).
Repository contracts and declarations do not establish live deployment.

## Selection and Confidentiality

Only the root project's `companion/index.html`, optional `companion/style.css`, and supported `companion/assets/` files supply browser presentation.
Companion files obey normal canonical UTF-8 normalization, revision hashing, and MCP mutation guards.
Dependencies do not supply companion content.
No endpoint exposes arbitrary project paths, Python, dependency files, generated `AGENTS.md`, requirements, or locks.

The browser receives selected sanitized presentation, not raw project files or a generic source interface.
This exception does not change the MCP-only boundary for all other source and project documentation.
Companion HTML, CSS, asset content, credentials, and tokens never enter logs, telemetry, errors, or browser diagnostics.
The host service serves trusted static bytes only; it never receives private content or tokens through HTTP.
The isolated browser frame receives the sanitized payload through a private message channel.

## Compiler Limits

| Bound | Limit |
| --- | --- |
| Combined normalized caller project content | 1,048,576 bytes, shared with all other project files |
| Companion namespace input | 524,288 bytes |
| Serialized compiled JSON output | 1,048,576 bytes |
| Assets | 64 |
| Parsed document nodes | 16,384 |
| Tree depth | 64 |
| Asset ID | 256 ASCII bytes |
| Runtime cut widgets per document | 16 |
| Runtime asset references per document | 128 |
| Aggregate runtime render budget | 16,384 nodes and 1,048,576 bytes |

Asset IDs start with `assets/` and end with lowercase `.svg` or `.json`.
Components contain ASCII letters, digits, `.`, `_`, or `-`; empty, dot, and parent components fail validation.
Unsupported files within `companion/` invalidate the companion when an index exists.
Without `companion/index.html`, no companion exists.

The compiler parses passive HTML and SVG through explicit element and attribute allowlists.
It accepts native document layout, local fragment links, inline SVG, and the optional kit elements.
It rejects authored executable content, event handlers, forms, frames, external navigation, `meta`, and `base`.
SVG references remain local; external resources and foreign executable namespaces fail validation.
Before `roxmltree` parses SVG, an iterative `xmlparser` preflight enforces the input, depth, and node limits.
The preflight rejects DTDs, entity declarations, processing instructions, and malformed structure before recursive parsing.
`roxmltree` also disables DTDs and bounds its node allocation.
The CSS parser rejects network resources, unsafe resource functions, and unsupported rules.
Passive `@page` and `@media print` rules remain valid, including supported nested print rules.
The runtime sanitizer, CSS filter, CSP, and sandbox provide independent defenses.
Runtime filtering can reject CSS beyond the compiler's accepted subset.

## Browser HTTP

`GET /artifacts/{model_id}/{revision}/companion` uses the current browser-session middleware in production.
An MCP bearer token does not substitute for a browser session.
Exact disabled-auth mode retains the local unauthenticated behavior.
Only the model's current-successful revision qualifies, including retained last-good state.

The successful `application/json` body has exactly six top-level fields:

```json
{"format":"faktory-companion-v1","revision":"<current-successful-project-sha256>","kit_version":"1","html":"<sanitized HTML>","css":"<validated CSS>","assets":[{"id":"assets/diagram.svg","mime_type":"image/svg+xml","content":"<sanitized SVG>"}]}
```

Each asset has exactly `id`, `mime_type`, and `content`.
SVG uses `image/svg+xml`; JSON uses `application/json`.
The route returns `404` for absent companions, absent success, or obsolete revisions.
Invalid companion content returns `422` with the constant safe error `companion contains unsupported or invalid content`.
No companion compile failure changes geometry readiness or render state.
Normal project edits still schedule CAD work; successful geometry selects the companion revision.

Authenticated `GET /runtime-config` returns `{"companion_origin":string|null}` with `Cache-Control: no-store`.
The server reads `FAKTORY_COMPANION_ORIGIN` at runtime.
The companion origin must be canonical and differ from the application origin.
HTTPS origins qualify; HTTP origins qualify only for localhost, loopback, or explicit disabled-auth local addresses.
Disabled mode also permits canonical RFC1918 IPv4, link-local addresses, and IPv6 unique-local addresses.
Production rejects HTTP on private non-loopback addresses.
Trusted local disabled-auth Compose uses the explicit private-network HTTP origins in [Companion Host Operation](../../operations/companion-host.md).
The SPA does not require a per-environment image build.
A null origin disables the frame integration without disabling geometry.

## Frame Boundary

The parent embeds `/frame` from the configured distinct companion origin.
The iframe uses `sandbox="allow-scripts allow-modals"`, `credentialless`, and a no-referrer policy.
It never grants `allow-same-origin`, forms, popups, top navigation, or downloads.
Its document has an opaque origin.
`allow-modals` exists solely for trusted print after an explicit parent user action.
Authors cannot invoke print or supply script.

The host exposes only `GET /frame`, `GET /health`, and `GET /ready` on port `8082`.
`COMPANION_PARENT_ORIGINS` supplies exact HTTP(S) origins separated by spaces.
The host rejects paths, wildcards, credentials, queries, and fragments as origin entries.
The frame accepts only messages whose source equals `parent` and whose origin appears in that allowlist.

The parent transfers one fresh `MessageChannel` with a fresh nonce and exact revision.
The handshake and channel messages bind both identities and validate their expected shape.
The frame accepts one content delivery; stale channels and callbacks cannot replace a newer document.
Disposal closes the channel and revokes asset object URLs.
The trusted runtime materializes sanitized SVG assets as local blobs.
JSON supplies passive content or the bounded cut widget.
The frame caps documents at 16 cut widgets and 128 asset references before expansion.
An aggregate render budget charges source HTML, CSS, foundation styles, custom-element expansion, asset materialization, widget output, and cached clones.
The budget permits at most 16,384 nodes and 1 MiB of charged bytes.
The runtime charges cached widget fragments and each clone before insertion.
It caches cut-widget output and SVG blobs by asset ID within one document.
Duplicate references do not repeat layout work or SVG blob allocation, but their output still consumes the aggregate budget.
The widget adapter estimates output nodes before table and diagram construction.
Budget failures reject the document rather than fall back to an optional widget error.

CSP permits only trusted nonce-authorized JavaScript.
`style-src 'unsafe-inline'` intentionally supports free-form local CSS and inline styles; styles do not require a nonce.
Compiler validation and the runtime CSS filter still reject unsafe CSS resources.
CSP forbids connections, child frames, objects, fonts, base URLs, and form actions.
The host applies no-store, nosniff, no-referrer, and restricted permissions headers.
It contains no authentication middleware, content-upload route, content persistence, or telemetry client.
[Companion host operation](../../operations/companion-host.md) defines the cookie-domain precautions.

## Cut Data

The widget accepts `{ "kind": "board" | "sheet", "input": ... }` from a JSON asset.
[`cuts.ts`](../../../packages/companion-kit/src/cuts.ts) owns the exported TypeScript data types and algorithms.
[`cut-widget.ts`](../../../packages/companion-kit/src/cut-widget.ts) owns the presentation adapter.

Both inputs require `units: "mm"`, finite `kerf` from zero through 1,000, and nonempty `stock` and `parts` arrays.
IDs must be strings with 1 through 64 ASCII letters, digits, `_`, or `-`.
IDs remain unique within each array.
Each dimension exceeds zero and does not exceed 1,000,000 mm.
Each `count` is an integer from 1 through 512.
There are at most 64 stock definitions, 128 part definitions, and 512 instances in each group.

Board stock and parts use `{id,length,count}`.
Board layouts use deterministic first-fit-decreasing lengths, without rotation.
The algorithm treats each board as a normalized unit-width strip, not an actual cross-section.
Authors supply compatible material and cross-sections independently.

Sheet stock uses `{id,width,height,count,grain}`.
Sheet parts use `{id,width,height,count,grain,rotate}`.
`grain` equals `none`, `x`, or `y`; `rotate` is a boolean.
Rotation swaps dimensions and the part grain axis.
A directional part requires compatible stock grain.

Two deterministic guillotine heuristics use horizontal-first and vertical-first splits.
The kit sorts valid alternatives by stock pieces, waste, and cut complexity.
These bounded heuristics do not prove global optimality or stock infeasibility after failure.
Validation checks stock availability, exact quantities, dimensions, rotation, grain, boundaries, non-overlap, kerf separation, and metrics.
The widget reports stock use, waste, cut complexity, placement tables, and an SVG layout.
Board metrics use millimetres; sheet metrics use square millimetres.
Waste includes kerf; cut complexity counts the heuristic's splits, not machine time.

No separate trim, miter-angle, thickness, or material field exists.
Supply net usable stock dimensions after trim allowances.
Supply miter part lengths between outside points.
Keep trim, material, cross-section, miter assumptions, and tool safety in the companion's explicit instructions.
Verify a practical cut order independently.
The kit supplies no geometry-derived cut list, structural assurance, manufacturing guarantee, or woodworking-only schema.
