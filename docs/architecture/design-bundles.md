# Multipart Design Bundles

## Status and Authority

This document defines the approved first-milestone multipart design-bundle contract. The branch implements this contract incrementally. Current executable paths can still accept one CadQuery value and emit one artifact set. Repository documentation and declarations do not prove a preview or production deployment.

This document owns result normalization, output identity, roles, primary selection, artifact eligibility, and exclusions. [`storage-rendering.md`](storage-rendering.md) owns immutable keys and atomic revision advancement. [`protocol.md`](protocol.md) owns MCP, protobuf, and HTTP exposure.

## Python Result API

The fixed renderer runtime provides the versioned source API `faktory_design.v1`:

```python
from faktory_design.v1 import Design, Output

result = Design(
    outputs=(
        Output(
            output_id="assembly",
            role="assembly",
            geometry=complete_assembly,
            primary=True,
        ),
        Output(
            output_id="bracket",
            role="part",
            geometry=bracket,
        ),
    )
)
```

`Design(outputs)` contains 1 through 64 `Output` entries. Each entry has exactly `output_id`, `role`, `geometry`, and `primary`. `primary` defaults to false. The API rejects unknown fields.

`output_id` is stable source-defined identity within the design. It contains at most 64 ASCII bytes and matches `^[a-z0-9]+(-[a-z0-9]+)*$`. IDs are unique, and declaration order is significant for deterministic manifests and API summaries. A design has exactly one primary output.

`role` accepts only `assembly`, `part`, or `tool`. The role describes the output's design purpose. It does not alter render rules or grant manufacturing semantics. `geometry` accepts one CadQuery `Workplane`, `Shape`, or `Assembly`.

The top-level project variable remains `result`. A direct CadQuery `Assembly`, `Workplane`, or `Shape` remains valid. Faktory normalizes an `Assembly` to one primary output with ID `primary` and role `assembly`. It normalizes a `Workplane` or `Shape` to one primary output with ID `primary` and role `part`. This compatibility form has the same all-or-nothing behavior as an explicit `Design`.

Output IDs, roles, order, and primary selection come from evaluated source. They do not alter canonical project revision identity. The project files, entrypoint, requirements, and exact locks remain the complete identity inputs. Source must produce the same design bundle for the same revision and locks. Nondeterministic artifact bytes can cause an immutable-write conflict and fail the render.

## Required Artifact Set

Every declared output must produce all of these revision artifacts:

- one GLB, capped at 64 MiB;
- one preview SVG;
- one geometry facts object;
- seven technical PNG projections.

The canonical Python-worker bundle is capped at 192 MiB in aggregate before server rasterization. The aggregate includes the compact `outputs.json` bytes and, for every declared output, `model.glb`, `preview.svg`, `facts.json`, and all seven technical projection SVGs. It is independent of, and does not replace, the 64 MiB limit on each GLB or any existing per-file validation. Unexpected files are outside the manifest, do not count toward the aggregate, and are never stored.

The Python worker sums only regular expected files with checked accumulation after the complete temporary bundle exists. It rejects and removes an over-limit temporary bundle before atomic publication. Rust independently sums the same exact expected files with checked accumulation after manifest validation and rejects an over-limit bundle before rasterization, shaded rendering, or complete in-memory output construction. Failures expose only safe generic renderer errors.

The server uses a disk-backed 512 MiB `/tmp`. After the 192 MiB output allowance, the remaining space covers the separately bounded 16 MiB canonical project, up to 64 directly locked libraries with at most 1 MiB of package content each, temporary-directory metadata, filesystem overhead, and atomic worker publication. Default render concurrency remains one. The aggregate worker cap plus bounded technical and shaded images keeps the validated Rust result within the server's 2 GiB memory limit.

Each facts object contains total volume in cubic millimetres and source-coordinate axis-aligned x/y/z dimensions in millimetres. Assembly and compound volume retains the current component-sum behavior, so overlaps can count independently.

Only the primary output receives seven canonical shaded projections. Named-view renders also use only the primary output. Non-primary outputs have no canonical shaded set and no named-view render cache in this milestone.

The complete immutable manifest is `outputs.json` with this canonical logical shape:

```json
{"format":"faktory-outputs-v1","outputs":[{"output_id":"assembly","role":"assembly","primary":true,"facts":{"volume_cubic_millimeters":1200.0,"size_millimeters":{"x":20.0,"y":10.0,"z":6.0}}}]}
```

The manifest contains exactly `format` and `outputs`. Each output summary contains exactly `output_id`, `role`, `primary`, and `facts`, in declared order. Each facts object contains exactly `volume_cubic_millimeters` and `size_millimeters`; size contains exactly `x`, `y`, and `z`. All values must pass the current finite, non-negative geometry validation. The stored manifest bytes use UTF-8 JSON without insignificant whitespace and end with one LF.

## Atomicity and Compatibility

All declared outputs are required. Faktory validates the manifest, aggregate worker-bundle capacity, every output's required artifacts, and the primary shaded set before any metadata advance. A missing, invalid, duplicate, oversized, or failed output fails the complete desired revision. Faktory does not publish a partial output set.

The current successful revision advances once after every immutable write succeeds. Its manifest, output summaries, primary facts alias, artifacts, and named-view basis advance together. A failure preserves the complete prior current-successful bundle. A first-render failure exposes no output manifest or artifacts.

Revisions created before this contract can lack `outputs.json`. Faktory interprets each such revision as one legacy primary output with ID `primary`. It infers role `assembly` because the legacy artifact does not retain the original Python result type. Legacy fixed keys remain readable through that synthetic output. Faktory performs no destructive migration or required backfill.

## Source-Level Design Semantics

Projects and shared libraries can expose ordinary Python helpers for constraints, interfaces, mating, placement, and output construction. These APIs remain source-level behavior under exact project and library locks. Faktory does not persist or interpret them as structured server constraints.

The [`constraint_bench` example](../../renderer/examples/constraint_bench/) demonstrates this source-level pattern. One frozen, validated interface specification supplies the nominal leg dimensions, fit clearance, and guide-bushing offset. The part generators derive the base slots from the leg cross-section and fit clearance, then derive the router-template opening from those slots and the guide-bushing offset. The primary bench assembly reuses one leg geometry at four placements; the design exposes one leg output and no quantity metadata. It emits the router template as a fabrication tool with role `tool` and does not emit construction references or keepout geometry. The values are illustrative inputs, not manufacturing guidance, and Faktory does not interpret their relationships.

Concrete hardware definitions belong in exact immutable shared-library releases. A change that affects physical geometry or fit is incompatible and requires a new major library release. Minor and patch releases can change documentation or behavior only when they remain geometry-compatible with every earlier release in that major. Faktory cannot infer physical compatibility from Python source; the publisher must choose the correct major version.

## Exclusions

This milestone adds no structured persisted constraints, server-side constraint solver, optional output failure, partial success, manufacturing export, or per-output named views. It adds no new project identity input, source exposure, untrusted-code sandbox, or mutable artifact semantics.
