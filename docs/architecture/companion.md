# Model Companions

A companion presents optional instructions, diagrams, measurements, tables, or material plans beside a model's successful geometry.
The kit serves general CAD projects, not only woodworking.
Authors control the passive HTML layout and local CSS; trusted kit components supply optional interaction.

[The companion specification](spec/companion.md) owns the transport, compiler, isolation, and cut-data contracts.
[Companion host operation](../operations/companion-host.md) owns configuration and deployment prerequisites.
Repository implementation does not prove a pipeline run or live deployment.

## Author Files

Create `companion/index.html` through the normal MCP project tools.
Add optional `companion/style.css` and UTF-8 SVG or JSON files below `companion/assets/`.
These files remain caller-owned canonical project files.
They share the project's combined 1 MiB normalized content limit with Python and other files.
Companion compilation also has separate limits in [the specification](spec/companion.md#compiler-limits).

Use semantic native HTML for the document structure.
Use local fragment links, tables with captions, useful diagram alternatives, and responsive CSS.
Native `details` and `summary` provide passive disclosure controls.
Inline SVG and SVG assets support diagrams without external requests.
The compiler rejects unsupported content instead of silently granting extra capabilities.

Do not include scripts, event attributes, forms, frames, external links, remote images, fonts, or network resources.
Do not include `meta` or `base` elements.
CSS resource syntax, imports, unsafe functions, and unsupported constructs fail compilation.
Passive `@page` and `@media print` rules support print layout.
The frame also applies a defensive HTML sanitizer and CSS filter.
Keep confidential Python, dependencies, guidance, and credentials outside the companion namespace.
Treat every companion asset as selected browser-visible presentation content.

## Generic Example

This `companion/index.html` describes a sensor bracket, without material-cut assumptions:

```html
<article>
  <h1>Sensor bracket</h1>
  <p>Check the sensor cable clearance before final installation.</p>
  <fk-measurement label="Mount spacing" value="40" unit="mm"></fk-measurement>
  <fk-tabs>
    <section data-tab="Installation">
      <fk-steps>
        <section><h2>Check alignment</h2><p>Align the bracket with the mounting surface.</p></section>
        <section><h2>Check clearance</h2><p>Verify the full cable bend radius.</p></section>
      </fk-steps>
    </section>
    <section data-tab="Inspection">
      <fk-callout>Confirm the fastener specification independently.</fk-callout>
      <table>
        <caption>Inspection points</caption>
        <thead><tr><th scope="col">Item</th><th scope="col">Requirement</th></tr></thead>
        <tbody><tr><td>Cable</td><td>No contact with the bracket edge</td></tr></tbody>
      </table>
    </section>
  </fk-tabs>
</article>
```

Optional `companion/style.css`:

```css
article { max-width: 60rem; margin: auto; }
h1 { color: #315d50; }
@media print { article { max-width: none; } }
```

Kit version `1` supplies these optional elements:

| Element | Author contract |
| --- | --- |
| `fk-tabs` | Direct child panels use `data-tab` labels; trusted code creates tab controls. |
| `fk-callout` | Passive highlighted content. |
| `fk-measurement` | `label`, `value`, and `unit` provide the displayed measurement. |
| `fk-steps` | Direct child `section` elements form numbered steps. |
| `fk-asset` | `data-companion-asset="assets/name.svg"` or a JSON asset selects local content. |
| `fk-cut-layout` | `data-companion-asset="assets/cuts.json"` selects an explicit cut request. |

An `img` also accepts `data-companion-asset` for an SVG asset and `alt` for its alternative.
Use the asset ID relative to `companion/`, not an HTTP URL or object-store key.

## Optional Cut Example

Include this element in the companion HTML:

```html
<fk-cut-layout data-companion-asset="assets/cuts.json"></fk-cut-layout>
```

Create `companion/assets/cuts.json` with explicit board data:

```json
{"kind":"board","input":{"units":"mm","kerf":3,"stock":[{"id":"board","length":2400,"count":2}],"parts":[{"id":"leg","length":700,"count":4}]}}
```

[The bench fixture](../../packages/companion-kit/fixtures/bench.json) shows the compiled presentation and this asset.
[The generic fixture](../../packages/companion-kit/fixtures/generic.json) demonstrates a non-woodworking companion.
Fixtures contain transport payloads; authors create project files, not a payload file with invented revision metadata.
[The cut-data specification](spec/companion.md#cut-data) defines board and sheet inputs, metrics, constraints, and limits.
The kit never infers a manufacturing cut list from geometry bounds.
Each document supports at most 16 cut widgets and 128 asset references.
Custom elements, assets, and widget output share a 16,384-node and 1 MiB runtime render budget.
Repeated references use cached layouts and SVG blobs; each rendered clone still consumes the budget.

## Revision Workflow

Read the model `/open` resource before an edit.
Use the current project revision for normal file-create and text-edit guards.
Every accepted companion edit creates a project revision and schedules a CAD render.
No companion-only revision or separate mutation API exists.
Follow the model's desired and current-successful revisions after each edit.

The browser selects the companion from the current-successful revision, including last-good geometry after a failed replacement.
The page warns when the desired revision differs.
Companion validation failure does not invalidate successful geometry.
An invalid companion produces a safe unavailable state; an absent companion leaves the ordinary model view usable.
The trusted parent print control prints the companion frame only after an explicit user action.
