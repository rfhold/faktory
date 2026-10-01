# View Tools and Image Semantics

All tools here accept exactly `{action, input}`. Place their domain arguments inside `input`; do not flatten them or add a filter.

```json
{"action":"view.render","input":{"model_id":"part","view_id":"saved-view-id"}}
```

| Tool | Input and behavior |
| --- | --- |
| Model `/images/{output_id}/{style}/{projection}` resource | Stored current-successful image; projection in isometric/front/back/left/right/top/bottom, style technical or shaded. Technical accepts any output; shaded accepts primary only. |
| Model `/views/{view_id}/image` resource | Existing cached shaded primary-only saved camera image; a miss returns not found, never renders. |
| `query` action `view.render` | `model_id`, `view_id`; on-demand shaded primary-only saved camera at exact current-successful revision and saved view etag. |
| Model `/views` and `/views/{view_id}` resources | Shared named views, canonical links, and etags. |
| `create` action `view.create` | `model_id`, `view`; empty/omitted view ID creates a new generated ID. No etag or existing ID is accepted. |
| `execute` action `view.update` | `model_id`, `view`, required `expected_etag`; existing ID updates conditionally. |
| `destroy` action `view.destroy` | `model_id`, `view_id`, required `expected_etag`. |
| `execute` action `view.set-default` | `model_id`, `view_id`; selects shared default. |

All input objects reject unknown fields. Model IDs are kebab-case ASCII at most 64 bytes; view IDs at most 64 bytes. `view` contains optional `id`, required `name` (1-200 UTF-8 bytes), `target` [x,y,z], `rotation` [x,y,z,w], `projection` PERSPECTIVE or ORTHOGRAPHIC, `distance`, `field_of_view_degrees`, `orthographic_scale`. Target and rotation must be finite; rotation magnitude must be finite and greater than machine epsilon, and is normalized by the server. Distance must be finite and positive. Perspective field of view must be finite and strictly between 0 and 180 degrees; orthographic scale must be finite and positive. Use exact opaque etags, never synthesize them.

Image resource reads return a bounded 640x480 `image/png` blob plus separate JSON metadata. On-demand query returns one semantic MCP image block plus text and structured metadata; do not treat a text-encoded URI or JSON image field as the image. Metadata distinguishes `desired_revision` from `rendered_revision` and `stale`; saved views include view etag and shaded recipe. Pending or failed desired revisions retain last-good images; no successful revision means no inspectable image. Saved image cache identity includes revision, primary output, view ID and etag; concurrent revision/view changes conflict rather than returning mismatched data.
