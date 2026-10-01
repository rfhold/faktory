# Model Projects and Dependencies

## Status and Authority

This document defines the implemented model-dependency hard cutover. Repository state does not prove a preview or production deployment.

The MCP API remains the only source-bearing interface. Protobuf and browser APIs expose only model metadata and current-successful artifacts. Existing protobuf fields such as `desired_source_revision` retain their names, but their values identify project revisions.

[`design-bundles.md`](design-bundles.md) owns the Python result contract. [`storage-rendering.md`](storage-rendering.md) owns object keys and render advancement. [`../operations/object-store-migrations.md`](../operations/object-store-migrations.md) owns the destructive cutover.

## Identity Types

Faktory keeps three identities distinct:

- A project revision identifies one immutable canonical project bundle by its lowercase SHA-256.
- A model release assigns one stable semantic version to one publishable project revision.
- A rendered output belongs to one successful project revision and does not affect project or release identity.

A model release never identifies rendered bytes. Rerendering an exact project revision does not create or alter a release.

## Canonical Project Bundle

A canonical bundle contains normalized UTF-8 project files, one explicit Python entrypoint, direct model requirements, and exact release locks. It includes the generated `AGENTS.md`.

The project can contain at most 256 caller-owned files and 64 direct model requirements. Each caller-owned file has a 1,048,576-byte limit. Their combined normalized content has a 1,048,576-byte limit. The generated `AGENTS.md` has a 1,048,576-byte limit. The final canonical bundle has a 16,777,216-byte limit. Empty files are valid, but the Python entrypoint must contain content.

Every path is a relative POSIX path. A path contains only printable ASCII bytes `0x20` through `0x7e` and uses `/` as its separator. A path has a 1,024-byte limit. Each component has a 255-byte limit. Paths are case-sensitive and unique by exact bytes. A path cannot be absolute or empty. It cannot contain an empty, `.` or `..` component, a backslash, or a trailing slash. `AGENTS.md` is reserved. Entries cannot represent directories, symlinks, hard links, or devices. The entrypoint names a current, nonempty `.py` file other than `AGENTS.md`.

Canonicalization validates paths without rewriting them. It decodes strict UTF-8, removes one leading UTF-8 BOM, converts all newlines to LF, and preserves final-newline presence. Faktory then regenerates `AGENTS.md`.

The canonical bundle uses UTF-8 JSON without insignificant whitespace and ends with one LF. Keys use the exact order below. JSON strings follow RFC 8259. The encoder escapes quotation marks, reverse solidus, and control characters. It uses defined two-character control escapes and lowercase `\u00xx` for other controls. It does not escape solidus or non-ASCII file content.

```json
{"format":"faktory-project-v2","entrypoint":"main.py","requirements":[{"model_id":"fasteners","range":">=1.2.0,<2.0.0"}],"locks":[{"model_id":"fasteners","version":"1.4.1","project_revision":"89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567","release_sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}],"files":[{"path":"AGENTS.md","content":"..."},{"path":"main.py","content":"..."}]}
```

Requirements and locks use ascending `model_id` ASCII-byte order. Files use ascending exact-path byte order. Each array has unique identities. Each direct requirement has one same-model lock. No other JSON member is valid. The project revision covers every canonical byte, including the final LF.

Only v2 project bundles are valid after the cutover. Upload order, request JSON, object metadata, model release versions, and rendered artifacts do not affect project revision identity.

## Requirements and Exact Locks

A model ID contains at most 64 ASCII bytes and matches `^[a-z0-9]+(-[a-z0-9]+)*$`. The only requirement range syntax is `>=MAJOR.MINOR.PATCH,<NEXT_MAJOR.0.0`, without whitespace. `NEXT_MAJOR` equals `MAJOR + 1`. Versions are stable `MAJOR.MINOR.PATCH` values without leading zeros, prerelease identifiers, or build metadata.

Initial resolution and explicit `dependencies.set` operations select the highest compatible published release for each direct requirement. Each lock records the selected model ID, version, project revision, and release SHA-256. Render and retry paths use stored exact locks and never consult the mutable release catalog.

Each released dependency project retains its own direct requirements and exact locks. Faktory traverses those immutable projects to create an exact transitive closure. Shared exact nodes count once.

Closure validation rejects:

- a cycle or self-dependency;
- an import across model namespaces without a direct requirement from the source project;
- two release identities or project revisions for the same model ID;
- a missing release, missing project bundle, or release whose bytes or identity are corrupt;
- more than 64 unique dependency models, excluding the root model;
- a path deeper than eight dependency edges from the root; or
- more than 64 MiB of materialized dependency package source after exact-node deduplication.

The source-size limit sums normalized bytes for files below `faktory_model/` in each unique dependency release. It excludes the root package and non-package dependency files. Faktory validates each cross-model import edge against that project's direct requirements; a project's own normalized namespace remains valid without a self-requirement. A project cannot import a transitive model unless it also declares that model directly. Faktory validates static imports before execution. Dynamic import tricks remain outside the trusted-source guarantee.

Faktory provides no external package declaration, installation, or resolution. Trusted projects can import the Python standard library and modules present in the fixed renderer runtime. That availability does not create a supported dependency contract. The renderer is not a hostile-code sandbox.

## Python Package Namespace

A publishable project contains `faktory_model/__init__.py`. Every file below `faktory_model/` follows the normal project path and size rules. Faktory exports that package under this exact namespace:

```text
faktory_models.m_<normalized_model_id>
```

Normalization replaces each `-` in `model_id` with `_` and makes no other change. The unconditional `m_` prefix makes leading digits and Python keywords safe. For example, model `3-way-clamp` exports `faktory_models.m_3_way_clamp`.

The renderer materializes the root package and every exact dependency package under `faktory_models`. Root entrypoints and dependency packages import only this final namespace. They never import through `faktory_model`. The source directory name is a publication layout, not an import alias.

The renderer executes only the root project's configured entrypoint. It never automatically executes a dependency's entrypoint. Ordinary Python import behavior can execute imported package modules.

## Managed AGENTS.md

Every project contains `AGENTS.md` with three top-level sections in this order:

1. `# Index` lists canonical project paths and identifies the entrypoint.
2. `# Dependency Guidance` lists each direct exact model release and its exact MCP read identity.
3. `# Hints` contains model-specific user guidance and can be empty.

Faktory owns the first two sections and their whitespace. The normalized `# Hints` body can contain headings at level two or lower. It cannot contain another top-level heading. Project creation accepts `hints`, not caller-supplied `AGENTS.md` bytes. Generic file operations cannot target `AGENTS.md`. `edit` on the dedicated `/hints` resource is its only public mutation interface.

Faktory regenerates the file after each file, entrypoint, requirement, or lock change. The index never exposes object keys or hidden metadata. Dependency entries use direct-lock order and include `model_id`, import package, version, project revision, release SHA-256, and an exact MCP resource URI instruction. With no locks, the dependency body is exactly `No model dependencies are locked.`.

The generated bytes use LF and end with one LF. Canonical JSON-string encoding quotes each path and model ID. The index includes `AGENTS.md` and uses canonical path order. Dependency sections use direct-lock order. This example defines the section shape:

Historical immutable v2 bundles retain their exact bytes and hashes. Validation reconstructs either the current resource-guidance shape or the exact historical tool-guidance shape. Both paths enforce canonical files, normalized hints, authenticated release locks, and complete bundle equality. Unknown, mixed, or modified guidance fails validation. Reads never migrate persisted bundles. New source revisions use resource guidance; old releases continue to identify the original historical revision.

```text
# Index

Entrypoint: "main.py"

- "AGENTS.md"
- "main.py"

# Dependency Guidance

## faktory_models.m_fasteners 1.4.1

Model: "fasteners"
Project revision: 89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567
Release: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef

Inspect exact files with MCP resources/read URI faktory://models/fasteners/releases/1.4.1.
Read one exact file with MCP resources/read URI faktory://models/fasteners/revisions/89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567/files/<percent-encoded-path>.

# Hints

<normalized user hints>
```

## Resource-First MCP Interface

Faktory registers exactly five tools: `create`, `edit`, `destroy`, `execute`, and `query`. Ordinary reads are resources, not query actions. `resources/list` discovers `faktory://models`; `resources/templates/list` discovers the model, project, file, release, view, and image templates. Collections link canonical item URIs. Public static Skills remain readable under `skill://` and discoverable through the Skills extension.

Under `faktory://models/{model_id}`, the item returns model metadata and collection links. `/project` returns the complete desired canonical project. `/open` returns metadata, actual revision, entrypoint, requirements, locks, full generated AGENTS.md, and file hashes with immutable links, without other bodies. `/files` returns the file index and exact immutable and desired edit URIs. `/files/{path}` reads desired text; `/revisions/{revision}/files/{path}` reads exact immutable text. `/hints` reads just the editable user Hints body. Text resources carry revision metadata. Paths use canonical uppercase percent encoding for all bytes except unreserved characters, including encoded path separators. Follow links; aliases, traversal, unknown targets, and arbitrary schemes are rejected.

`/releases` links immutable release items; `/releases/{version}` returns release metadata, exact closure, namespace, and file hashes with immutable source links. `/views` links saved-view metadata and image resources; `/views/{view_id}` returns the exact saved camera and etag. `/views/{view_id}/image` reads an existing cached image only. `/images/{output_id}/{style}/{projection}` reads an existing last-successful technical or shaded projection. Image resources use binary `image/png` blobs and separate JSON metadata, not text-encoded image fields. Resource reads never invoke the visual renderer. Missing images return not found. Desired/rendered revision and staleness remain distinct.

Create, destroy, execute, and query accept exactly `{action, input}`. They take a required `action` discriminator in `domain.operation` form, with action-specific arguments inside a required `input` object. Every input object rejects unknown fields.

```json
{"action":"file.create","input":{"model_id":"part","expected_revision":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","path":"lib/part.py","content":"value = 1"}}
```

No tool accepts an arbitrary filter. File templates use ordinary `{path}` expansion, not reserved `{+path}` expansion. The whole path forms one encoded component: `nested/a#b:c%.py` expands to `nested%2Fa%23b%3Ac%25.py`. Raw separators and alternate encodings are not canonical aliases.

| Tool | Actions and arguments |
| --- | --- |
| `create` | `model.create`: `model_id`, `name`, `files`, `entrypoint`, optional `requirements` and `hints`; `file.create`: `model_id`, `expected_revision`, `path`, `content`; `view.create`: `model_id`, `view` with an empty/omitted ID and no etag. Existing model IDs and file paths are never upserted; view IDs are server-generated. |
| `destroy` | `file.destroy`: `model_id`, `expected_revision`, `path`; `view.destroy`: `model_id`, `view_id`, `expected_etag`. Only existing caller files and saved views can be removed. Generated AGENTS.md, the entrypoint, models, and releases cannot be deleted. |
| `execute` | `model.set-name`: `model_id`, `expected_revision`, `name`; `file.rename`: `model_id`, `expected_revision`, `from`, `to`; `entrypoint.set`: `model_id`, `expected_revision`, `path`; `dependencies.set`: `model_id`, `expected_revision`, `requirements`; `view.update`: `model_id`, `view` with existing ID, `expected_etag`; `view.set-default`: `model_id`, `view_id`; `model.render.retry`: `model_id`; `model.release.publish`: `model_id`, `version`, `expected_revision`. |
| `query` | `model.glob`: `model_id`, `pattern`, optional `revision`; `model.grep`: `model_id`, `pattern`, optional `include`, `revision`, and `limit`; `view.render`: `model_id`, `view_id`. Search bounds remain unchanged. Only the saved-view query may render on demand and returns a semantic MCP image block. |

`edit` has exactly this text-only input, with no action or metadata operations:

```text
{uri: string, expected_revision: string, edits: [
  {operation: "replace", old_text: string, new_text: string} |
  {operation: "insert", text: string,
   placement: "start" | "end" | "before" | "after", anchor?: string}
]}
```

The URI must name a desired caller file or the model's `/hints` body, never generated AGENTS.md, an immutable file, or a non-text object. The exact desired project revision guards the complete transaction. One through 256 edits execute in order and commit atomically. Replace requires nonempty old text matching exactly once, counting overlapping matches. Insert requires nonempty text; before/after require a nonempty unique anchor, while start/end reject an anchor. No-op edits and a transaction with unchanged canonical content are rejected. Empty caller files and empty Hints accept insertions. Intermediate text remains bounded to 1,048,576 bytes, and canonical project limits and normalization still apply. Locks and generated sections remain server-owned.

Canonical no-op detection compares normalized caller files, Hints, entrypoint, requirements, and authenticated locks independently of generated guidance versions. A legacy-to-resource instruction change alone cannot create a revision. This applies to text edits and unchanged entrypoint/dependency executions. A rejected canonical no-op can wait for and reserve existing render admission capacity, but releases it without a commit or render submission.

A successful source change schedules one render. Cancellation of a caller after the spawned commit starts does not cancel the commit or its reserved render submission. Name-only execution does not change the project revision or schedule rendering. Conflict requires rereading the resource and recomputing the edits. Coordination remains single-replica; this interface does not add distributed mutation locking.

The old ordinary read, edit, and stripped-patch tools are not registered. There is no singular `source` input or output, filesystem access, arbitrary object-store access, or alternate source-bearing browser interface.

## Immutable Model Releases

Any model can publish when its desired revision equals its current-successful revision and its render state is `READY`. The selected project must contain the required `faktory_model/__init__.py`. Publication does not include rendered outputs.

A release record has this canonical logical shape:

```json
{"format":"faktory-model-release-v1","model_id":"fasteners","version":"1.4.1","project_revision":"89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567"}
```

The canonical record follows the project JSON string rules, contains no unspecified members, and ends with one LF. Its lowercase SHA-256 is `release_sha256`. The immutable project bundle remains the source and documentation payload.

Publication verifies the complete exact dependency closure before it writes the release. Versions increase globally and monotonically for each model. A new version must exceed every version already published for that model. Republishing one version with byte-identical canonical release bytes succeeds idempotently. The same version with different bytes conflicts. Releases cannot change, disappear, move to another project revision, or receive a yank marker.

Model authors own semantic compatibility. Minor and patch releases must preserve compatibility with every earlier release in that major. Any breaking Python API change requires a new major version. Any change to physical geometry, fit, clearance, mating interfaces, or other physical behavior requires a new major version. Faktory cannot infer these compatibility properties from source.

MCP reads releases through the `/releases` collection and exact `/releases/{version}` resource. `execute` action `model.release.publish` publishes an exact READY revision and preserves byte-identical republication semantics. Publication is not a text edit or a create/upsert operation. Exact source and documentation use linked `/revisions/{project_revision}/files/{path}` resources. There is no mutable release default. Protobuf, browser, and artifact HTTP APIs remain dependency-blind and source-free.

## Compatible Release Rollout

A new minor or patch release creates a durable rollout record keyed by model ID and exact release identity. The single server processes each record idempotently.

For each current consumer whose direct range contains the release, Faktory rechecks the desired project under its mutation lock. It replaces only that direct lock, validates the exact closure, regenerates `AGENTS.md`, calculates a deterministic project revision, and marks that revision `PENDING`. The normal render path preserves last-good outputs and advances them atomically.

Each rollout records a terminal outcome per consumer and resumes after restart. It does not duplicate project revisions or render work. If a consumer changes concurrently, Faktory re-evaluates its new desired project. Publication succeeds after the release and rollout intent become durable. It does not wait for consumer renders.

A later compatible release remains eligible after an earlier consumer render failure. A new major release never changes a consumer. A consumer adopts a new major through an explicit `dependencies.set` range change. That operation provides the supported path for SemVer-breaking and physical/API-breaking adoption.

## Confidentiality and Exclusions

Project files, dependency source, generated guidance, requirements, and locks remain MCP-only. Model metadata and image resources, protobuf records, browser APIs, HTTP artifacts, logs, telemetry, and safe errors never contain that content.

Project tools and release tools operate over canonical objects. They do not expose server filesystem paths or arbitrary object keys. The contract adds no external dependency manager, release mutation, deletion, yank, prerelease version, hostile-code sandbox, or multi-replica coordination.
