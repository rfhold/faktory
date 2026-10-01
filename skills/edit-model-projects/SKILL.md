---
name: edit-model-projects
description: Create, discover, read, and conditionally edit Faktory multi-file CadQuery model projects; use for source, entrypoint, dependency, or model hint changes.
---
# Edit Model Projects

1. Discover resources and templates. Read `faktory://models`, follow a model URI, then read its `/open` resource before selective discovery. Read generated `AGENTS.md`, entrypoint, requirements, locks, and file hashes.
2. Read [project tools and bounds](references/tools.md). Follow canonical immutable file links. Use `query` actions `model.glob` and `model.grep` for bounded searches pinned to the returned revision. Read `/project` only when the complete canonical project is needed.
3. Guard text changes with `expected_revision`. Use `edit` on a desired file URI or `/hints`, with ordered replace/insert edits. Use `create`, `destroy`, or `execute` for non-text operations. On conflict reopen and recompute; never overwrite blindly.
4. Follow render state through the model resource. Read stored image resources without rendering; do not confuse last-good images with the desired revision. Retry only a failed desired revision with `execute` action `model.render.retry`.

Static skills are procedural guidance, not permission to mutate. Generated project `AGENTS.md` remains revision-, path-, and dependency-lock-specific canonical data; it is not replaced by this skill. Its Index and Dependency Guidance are protected; only the `/hints` text resource is editable. Do not expose project source outside authenticated MCP.
