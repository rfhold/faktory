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

Faktory owns the first two sections and their whitespace. The normalized `# Hints` body can contain headings at level two or lower. It cannot contain another top-level heading. Project creation accepts `hints`, not caller-supplied `AGENTS.md` bytes. Generic file operations cannot target `AGENTS.md`. `hints.patch` is its only mutation operation.

Faktory regenerates the file after each file, entrypoint, requirement, or lock change. The index never exposes object keys or hidden metadata. Dependency entries use direct-lock order and include `model_id`, import package, version, project revision, release SHA-256, and an exact `model.read` instruction. With no locks, the dependency body is exactly `No model dependencies are locked.`.

The generated bytes use LF and end with one LF. Canonical JSON-string encoding quotes each path and model ID. The index includes `AGENTS.md` and uses canonical path order. Dependency sections use direct-lock order. This example defines the section shape:

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

Inspect exact files with MCP model.release.get(model_id="fasteners", version="1.4.1").
Read one exact file with MCP model.read(model_id="fasteners", revision="89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567", path=<path>).

# Hints

<normalized user hints>
```

## MCP Project Tools

The cutover retains the bulk `model.create`, `model.get`, and `model.edit` contracts. It retains agent-oriented project tools. It accepts no singular `source` or source-patch schema.

- `model.create` requires `model_id`, `name`, `files`, and `entrypoint`. Optional `requirements` and `hints` default to an empty array and empty string. Faktory resolves requirements before it calculates the project revision.
- `model.get` accepts only `model_id`. It returns complete model metadata and the complete desired project, including all files, `AGENTS.md`, entrypoint, requirements, and exact locks.
- `model.edit` requires `model_id`, `expected_revision`, and a name change or an `operations` array. The array has 1 through 256 ordered operations. It supports `file.add`, `file.patch`, `file.delete`, `file.rename`, `entrypoint.set`, `dependencies.set`, and `hints.patch`. A name-only edit omits `operations`.
- `model.open` returns desired metadata, revision, entrypoint, requirements, locks, complete `AGENTS.md`, and a content-hash file index. It omits other file bodies.
- `model.read` reads one file from the desired or supplied exact project revision. Release source and docs use the release's exact `project_revision` with this operation.
- `model.glob` and `model.grep` provide bounded discovery within the desired or supplied exact project revision.
- `model.apply_patch` applies one stripped patch envelope to the desired revision as one guarded transaction.
- `model.render.retry` retries the exact desired project revision and lock closure. It does not resolve releases or change revision identity.

All input objects reject unknown fields. Existing operation ordering, exact-patch, line-read, glob, grep, and stripped-patch semantics remain unchanged. A successful source mutation creates one immutable project revision and schedules one render.

Agents use `model.open` before selective discovery. They use the returned revision to guard later reads and mutations. They use `model.edit` for metadata, entrypoints, dependencies, hints, or structured file operations. They use `model.get` only for a complete project body.

## Immutable Model Releases

Any model can publish when its desired revision equals its current-successful revision and its render state is `READY`. The selected project must contain the required `faktory_model/__init__.py`. Publication does not include rendered outputs.

A release record has this canonical logical shape:

```json
{"format":"faktory-model-release-v1","model_id":"fasteners","version":"1.4.1","project_revision":"89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567"}
```

The canonical record follows the project JSON string rules, contains no unspecified members, and ends with one LF. Its lowercase SHA-256 is `release_sha256`. The immutable project bundle remains the source and documentation payload.

Publication verifies the complete exact dependency closure before it writes the release. Versions increase globally and monotonically for each model. A new version must exceed every version already published for that model. Republishing one version with byte-identical canonical release bytes succeeds idempotently. The same version with different bytes conflicts. Releases cannot change, disappear, move to another project revision, or receive a yank marker.

Model authors own semantic compatibility. Minor and patch releases must preserve compatibility with every earlier release in that major. Any breaking Python API change requires a new major version. Any change to physical geometry, fit, clearance, mating interfaces, or other physical behavior requires a new major version. Faktory cannot infer these compatibility properties from source.

The MCP model-release operations are:

- `model.release.list(model_id)` returns stable versions and exact release metadata without project file bodies.
- `model.release.publish(model_id, version, expected_revision)` publishes the exact current READY project revision.
- `model.release.get(model_id, version)` returns exact version, project revision, release SHA-256, package namespace, complete exact closure identities, and a content-hash project file index. It omits project file bodies.

Agents use `model.read(model_id, path, revision=project_revision)` for exact release source or documentation. There is no mutable release default. Model release operations remain MCP-only. Protobuf, browser, and artifact HTTP APIs remain dependency-blind and source-free.

## Compatible Release Rollout

A new minor or patch release creates a durable rollout record keyed by model ID and exact release identity. The single server processes each record idempotently.

For each current consumer whose direct range contains the release, Faktory rechecks the desired project under its mutation lock. It replaces only that direct lock, validates the exact closure, regenerates `AGENTS.md`, calculates a deterministic project revision, and marks that revision `PENDING`. The normal render path preserves last-good outputs and advances them atomically.

Each rollout records a terminal outcome per consumer and resumes after restart. It does not duplicate project revisions or render work. If a consumer changes concurrently, Faktory re-evaluates its new desired project. Publication succeeds after the release and rollout intent become durable. It does not wait for consumer renders.

A later compatible release remains eligible after an earlier consumer render failure. A new major release never changes a consumer. A consumer adopts a new major through an explicit `dependencies.set` range change. That operation provides the supported path for SemVer-breaking and physical/API-breaking adoption.

## Confidentiality and Exclusions

Project files, dependency source, generated guidance, requirements, and locks remain MCP-only. Model list and inspect tools, protobuf records, browser APIs, HTTP artifacts, logs, telemetry, and safe errors never contain that content.

Project tools and release tools operate over canonical objects. They do not expose server filesystem paths or arbitrary object keys. The contract adds no external dependency manager, release mutation, deletion, yank, prerelease version, hostile-code sandbox, or multi-replica coordination.
