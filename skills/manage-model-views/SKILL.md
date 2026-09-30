---
name: manage-model-views
description: Inspect Faktory technical or shaded model images and create, update, delete, or choose shared named views; use for saved cameras, image review, and view conflicts.
---
# Manage Model Views

1. Read [view tools and image semantics](references/views.md). Use `model.list` or `model.open` to identify the model and desired/current-successful render state.
2. Inspect with `model.inspect` for canonical projections or `view.inspect` for an exact saved camera. Read the semantic image block and metadata together; a last-good image can be stale.
3. Use `view.list` before mutation. Create through `view.put` without an ID; update with the saved ID and exact `expected_etag`. Re-read after conflict and recompute.
4. Use `view.set-default` to choose the shared default; delete only with explicit authority and the exact saved `expected_etag` via `view.delete`.

Views are shared data, not private preferences. Skills do not grant mutation permission. Source projects and dependency guidance remain MCP-only, and images never prove the desired revision has rendered.
