# Project Tools and Bounds

All inputs reject unknown fields. Model IDs are kebab-case ASCII, at most 64 bytes. Revisions are exact 64-character lowercase SHA-256 identities. Files use strict relative printable-ASCII POSIX paths, at most 1024 bytes and 255 bytes per component; no absolute paths, backslashes, empty, dot, or parent components.

| Tool | Input and behavior |
| --- | --- |
| `model.list` | Empty object; metadata only. |
| `model.open` | `model_id`; desired revision, file hashes, full generated AGENTS.md, entrypoint, requirements, locks; other bodies omitted. |
| `model.get` | `model_id`; complete desired canonical bundle, not last-good source. |
| `model.read` | `model_id`, `path`; optional exact `revision`, one-based `offset` default 1, `limit` default 200, maximum 2000 lines. Returns actual revision and truncation. |
| `model.glob` | `model_id`, ASCII `pattern` (1-1024 bytes), optional `revision`; at most 1000 paths. `*`, `?`, classes do not cross `/`; valid `**` recurses. |
| `model.grep` | `model_id`, Rust regex `pattern` (1-1024 UTF-8 bytes), optional ASCII `include` glob and `revision`; `limit` default 100, maximum 1000; matching text at most 2000 UTF-8 bytes per line. |
| `model.create` | `model_id`, `name` (1-200 UTF-8 bytes), `files`, `entrypoint`; optional `requirements` default [], `hints` default empty. No legacy source/dependencies fields. |
| `model.edit` | `model_id`, `expected_revision`, and `name` or `operations` (1-256). Executes operations in array order atomically. |
| `model.apply_patch` | `model_id`, `expected_revision`, `patch` (1-1048576 UTF-8 bytes). Parse the entire Begin/End Patch envelope first; ordered Add/Update/Move/Delete sections, exact unique hunks, protected AGENTS.md; one transaction and one scheduled render. |
| `model.render.retry` | `model_id`; retries failed desired revision with stored exact lock closure, never re-resolves releases or changes revision identity. |

Creation accepts 1-256 caller files, each and combined normalized content at most 1048576 bytes. Entrypoint is an existing nonempty `.py` file. At most 64 direct requirements; each range is exactly `>=MAJOR.MINOR.PATCH,<NEXT_MAJOR.0.0`, stable versions only. The server resolves highest compatible releases on creation or explicit `dependencies.set`; callers never supply locks. Packages import through `faktory_models.m_<model_id_with_hyphens_replaced_by_underscores>`, never `faktory_model`. Cross-model imports require a direct requirement; transitive-only imports are invalid. No external package installer or hostile-code sandbox exists.

Ordered operations: `file.add(path,content)`, `file.patch(path,patches)`, `file.delete(path)`, `file.rename(from,to)`, `entrypoint.set(path)`, `dependencies.set(requirements)`, `hints.patch(patches)`. Exact patches are 1-256 `{old,new}` objects with nonempty `old` matching exactly once. Generic file edits cannot target AGENTS.md. Hints allow headings level two or lower, not top-level headings. Name-only edits omit operations and do not schedule source rendering.

Canonical UTF-8 normalization removes one leading BOM, normalizes LF, and preserves final-newline presence. Hashes cover the canonical bundle including generated AGENTS.md, requirements, and locks. Never hand-recalculate or override a server revision. Closure bounds are 64 dependency models, eight edges deep, and 64 MiB materialized package source; cycles, self-dependencies, conflicting identities, missing or corrupt releases fail.

Generated AGENTS.md is limited to 1048576 bytes, and the complete final canonical bundle to 16777216 bytes. Caller-owned empty files are valid except for the entrypoint. Paths and exact file identity are case-sensitive and are validated, not rewritten; no symlink, directory, or device entries are supported.
