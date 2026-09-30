# View Tools and Image Semantics

| Tool | Input and behavior |
| --- | --- |
| `model.inspect` | `model_id`, `projection` in isometric/front/back/left/right/top/bottom; optional `output_id` defaults primary, `render_style` technical (default) or shaded. |
| `view.inspect` | `model_id`, `view_id`; shaded primary-only saved camera at exact current-successful revision and saved view etag. |
| `view.list` | `model_id`; shared named views and etags. |
| `view.put` | `model_id`, `view`, optional `expected_etag`; empty/omitted view ID creates, existing ID updates conditionally. |
| `view.delete` | `model_id`, `view_id`, required `expected_etag`. |
| `view.set-default` | `model_id`, `view_id`; selects shared default. |

All input objects reject unknown fields. Model IDs are kebab-case ASCII at most 64 bytes; view IDs at most 64 bytes. `view` contains optional `id`, required `name` (1-200 UTF-8 bytes), `target` [x,y,z], `rotation` [x,y,z,w], `projection` PERSPECTIVE or ORTHOGRAPHIC, `distance`, `field_of_view_degrees`, `orthographic_scale`. Target and rotation must be finite; rotation magnitude must be finite and greater than machine epsilon, and is normalized by the server. Distance must be finite and positive. Perspective field of view must be finite and strictly between 0 and 180 degrees; orthographic scale must be finite and positive. Use exact opaque etags, never synthesize them.

Inspection returns one bounded 640x480 PNG as an MCP image block, plus text and structured metadata; do not treat a text-encoded URI or JSON image field as the image. Technical inspection supports any output; shaded canonical and saved-view inspection support only primary. Metadata distinguishes `desired_revision` from `rendered_revision`, render state, output identity, and `stale`; saved views include view etag and shaded recipe. Pending or failed desired revisions retain last-good images with a warning; no successful revision means no inspectable image. Saved image cache identity includes revision, primary output, view ID and etag; concurrent revision/view changes conflict rather than returning mismatched data.
