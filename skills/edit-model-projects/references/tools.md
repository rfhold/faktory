# Project Tools and Bounds

All inputs reject unknown fields. Model IDs are kebab-case ASCII, at most 64 bytes. Revisions are exact 64-character lowercase SHA-256 identities. Files use strict relative printable-ASCII POSIX paths, at most 1024 bytes and 255 bytes per component; no absolute paths, backslashes, empty, dot, or parent components.

Non-edit tools accept exactly `{action, input}`. Place the action-specific fields in `input`; do not flatten them or add a filter. `edit` accepts its direct text-edit object without an envelope.

Historical immutable AGENTS.md bytes can name retired tools. Use current release/file resource links instead; never rewrite historical guidance or recalculate its stored revision.

```json
{"action":"model.glob","input":{"model_id":"part","revision":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","pattern":"**/*.py"}}
```

File templates use ordinary `{path}` expansion for the whole nested path as one encoded component. For example, `nested/a#b:c%.py` becomes `nested%2Fa%23b%3Ac%25.py`. Reserved expansion, raw separators, and lowercase percent-encoding aliases are not accepted.

| Interface | Input and behavior |
| --- | --- |
| `resources/read` `faktory://models` | Metadata collection with canonical model links. |
| Model `/open` resource | Desired revision, file hashes and immutable links, full generated AGENTS.md, entrypoint, requirements, locks; other bodies omitted. |
| Model `/project` resource | Complete desired canonical bundle, not last-good source. |
| Model `/files` and `/revisions/{revision}/files/{path}` resources | File index with immutable links; full bounded UTF-8 file text and actual revision metadata. The whole path, including `/`, occupies one encoded URI component using canonical uppercase percent encoding; follow returned links. |
| `query` action `model.glob` | `model_id`, ASCII `pattern` (1-1024 bytes), optional `revision`; at most 1000 paths. `*`, `?`, classes do not cross `/`; valid `**` recurses. |
| `query` action `model.grep` | `model_id`, Rust regex `pattern` (1-1024 UTF-8 bytes), optional ASCII `include` glob and `revision`; `limit` default 100, maximum 1000; matching text at most 2000 UTF-8 bytes per line. |
| `create` action `model.create` | `model_id`, `name` (1-200 UTF-8 bytes), `files`, `entrypoint`; optional `requirements` default [], `hints` default empty. No legacy source/dependencies fields. Existing IDs conflict. |
| `create` action `file.create` | `model_id`, `expected_revision`, `path`, `content`; existing paths are rejected, never replaced. |
| `edit` | Exactly `uri`, `expected_revision`, `edits` (1-256). Desired file or hints URI only; atomic text operations. |
| `destroy` action `file.destroy` | `model_id`, `expected_revision`, `path`; generated AGENTS.md and the entrypoint cannot be removed. |
| `execute` | `model.set-name(model_id,expected_revision,name)`, `file.rename(model_id,expected_revision,from,to)`, `entrypoint.set(model_id,expected_revision,path)`, `dependencies.set(model_id,expected_revision,requirements)`. |
| `execute` action `model.render.retry` | `model_id`; retries failed desired revision with stored exact lock closure, never re-resolves releases or changes revision identity. |

Creation accepts 1-256 caller files, each and combined normalized content at most 1048576 bytes. Entrypoint is an existing nonempty `.py` file. At most 64 direct requirements; each range is exactly `>=MAJOR.MINOR.PATCH,<NEXT_MAJOR.0.0`, stable versions only. The server resolves highest compatible releases on creation or explicit `dependencies.set`; callers never supply locks. Packages import through `faktory_models.m_<model_id_with_hyphens_replaced_by_underscores>`, never `faktory_model`. Cross-model imports require a direct requirement; transitive-only imports are invalid. No external package installer or hostile-code sandbox exists.

Each text edit is `{operation:"replace",old_text,new_text}` or `{operation:"insert",text,placement,anchor?}`. Replace requires a nonempty unique match, counting overlaps. Insert requires nonempty text. Start/end placements reject anchors; before/after require a nonempty unique anchor. Edits run in array order, reject no-ops, and commit as one revision with one render. Generic file edits cannot target AGENTS.md or immutable revision URIs. Edit `/hints` for the user body, including empty hints. Hints allow headings level two or lower, not top-level headings. Name-only execution does not schedule source rendering.

Canonical UTF-8 normalization removes one leading BOM, normalizes LF, and preserves final-newline presence. Hashes cover the canonical bundle including generated AGENTS.md, requirements, and locks. Never hand-recalculate or override a server revision. Closure bounds are 64 dependency models, eight edges deep, and 64 MiB materialized package source; cycles, self-dependencies, conflicting identities, missing or corrupt releases fail.

Generated AGENTS.md is limited to 1048576 bytes, and the complete final canonical bundle to 16777216 bytes. Caller-owned empty files are valid except for the entrypoint. Paths and exact file identity are case-sensitive and are validated, not rewritten; no symlink, directory, or device entries are supported.

## Companion Author Guidance

Companions serve general CAD projects, not only woodworking.
Create optional `companion/index.html`, `companion/style.css`, and UTF-8 `.svg` or `.json` files below `companion/assets/`.
Use normal `file.create`, text `edit`, and `file.destroy` operations with the current desired revision guard.
Every accepted companion edit changes the canonical project revision and schedules CAD work.
No special tool, uploaded browser file, authored script, or geometry-bounds inference exists.

Use native passive HTML and local CSS for a free-form layout.
Do not include events, forms, frames, external links, remote resources, `meta`, or `base`.
Passive `@page` and `@media print` CSS rules support print layout.
Inline SVG supports diagrams; asset IDs use `assets/name.svg` or `assets/name.json`.
Keep Python, dependencies, guidance, credentials, and secrets outside the selected companion namespace.
The browser receives only this sanitized presentation from the current-successful revision, not generic project source.

Minimal non-woodworking `companion/index.html`:

```html
<article>
  <h1>Sensor bracket</h1>
  <fk-callout>Confirm cable clearance before installation.</fk-callout>
  <fk-measurement label="Mount spacing" value="40" unit="mm"></fk-measurement>
  <details><summary>Inspection</summary><p>Check that the cable clears the bracket edge.</p></details>
</article>
```

Optional kit version `1` components also include `fk-tabs`, `fk-steps`, `fk-asset`, and `fk-cut-layout`.
Tab panels use `data-tab`; steps use direct child `section` elements.
Assets use `data-companion-asset` with a local asset ID.
Native HTML remains valid without kit-specific layout.

Optional explicit board example:

```html
<fk-cut-layout data-companion-asset="assets/cuts.json"></fk-cut-layout>
```

`companion/assets/cuts.json`:

```json
{"kind":"board","input":{"units":"mm","kerf":3,"stock":[{"id":"board","length":2400,"count":2}],"parts":[{"id":"leg","length":700,"count":4}]}}
```

Sheet requests use `kind:"sheet"` with `units`, `kerf`, `stock`, and `parts` in `input`.
Stock entries use `id,width,height,count,grain`; parts also require boolean `rotate`.
Grain is `none`, `x`, or `y`.
Use explicit millimetres and quantities; never derive cut lengths from bounding boxes.
Supply net stock after trim and miter lengths between outside points.
No separate trim or miter field exists.
The deterministic board and guillotine heuristics validate placements and report stock use, waste, and cut complexity.
They do not promise global optimality or safe machine cut order.

The companion namespace has a 524288-byte input budget, 1048576-byte compiled output limit, and at most 64 assets.
Parsed documents have at most 16384 nodes and depth 64.
SVG preflight applies these bounds before recursive parsing and rejects DTDs and entity declarations.
Runtime documents allow at most 16 cut widgets and 128 asset references.
An aggregate runtime budget allows 16384 nodes and 1048576 charged bytes across source, styles, assets, components, widgets, and cached clones.
Duplicate references reuse layout and SVG blob caches; each clone still consumes the budget.
All caller files still share the project's 1048576-byte normalized content limit.
Invalid companions leave successful geometry intact.
Pending or failed replacements retain last-good presentation and a stale warning.
The trusted parent prints through an explicit user action; authors cannot supply print scripts.
