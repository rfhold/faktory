---
name: edit-model-projects
description: Create, discover, read, and conditionally edit Faktory multi-file CadQuery model projects; use for source, entrypoint, dependency, or model hint changes.
---
# Edit Model Projects

1. Use `model.list` to find a model, then `model.open` before selective discovery. Read its generated `AGENTS.md`, entrypoint, requirements, exact locks, and file hashes.
2. Read [project tools and bounds](references/tools.md). Pin `model.read`, `model.glob`, and `model.grep` to the returned revision. Use `model.get` only when the complete canonical project is needed.
3. Guard changes with `expected_revision`. Use `model.apply_patch` for caller files or `model.edit` for ordered operations, metadata, entrypoints, dependencies, and Hints. On conflict reopen and recompute; never overwrite blindly.
4. Follow render state with `model.open`. Inspect successful output with `model.inspect`; do not confuse last-good images with the desired revision. Retry only a failed desired revision using `model.render.retry`.

Static skills are procedural guidance, not permission to mutate. Generated project `AGENTS.md` remains revision-, path-, and dependency-lock-specific canonical data; it is not replaced by this skill. Its Index and Dependency Guidance are protected; only `hints.patch` changes user Hints. Do not expose project source outside authenticated MCP.
