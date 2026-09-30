---
name: publish-model-releases
description: Inspect and permanently publish immutable Faktory model releases and adopt dependency major versions; use for release readiness, compatibility, and exact released-source reads.
---
# Publish Model Releases

1. Read [release tools and compatibility](references/releases.md), then `model.open` and `model.release.list` for the requested model.
2. Verify publishable package layout and successful output. Require `READY` with desired revision equal to current-successful revision. Publication is permanent; obtain explicit authority for the model, version, and exact revision.
3. Choose a globally increasing stable version. Breaking Python APIs or any physical geometry, fit, clearance, or mating-interface change requires a new major.
4. Call `model.release.publish` with the verified `expected_revision`. Record release identity and rollout outcome separately from consumer rendering; publication does not wait for renders.
5. For dependency adoption, inspect `model.release.get`, then pin `model.read` to its exact project revision. Change a consumer's major range only through guarded `model.edit` with `dependencies.set`.

Skills never grant publication authority. Keep dependency source and generated project guidance MCP-only; never substitute static skill content for revision-specific AGENTS.md or exact locks.
