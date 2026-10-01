---
name: manage-model-views
description: Inspect Faktory technical or shaded model images and create, update, delete, or choose shared named views; use for saved cameras, image review, and view conflicts.
---
# Manage Model Views

1. Read [view tools and image semantics](references/views.md). Read `faktory://models` and follow the model URI to identify desired/current-successful render state.
2. Follow canonical stored projection or saved-view image resources. Resource reads never render a missing image; use `query` action `view.render` for on-demand saved-camera rendering. Read image content and metadata together; a last-good image can be stale.
3. Read the model's `/views` resource before mutation. Use `create` action `view.create` without an ID; update with `execute` action `view.update`, the saved ID, and exact `expected_etag`. Re-read after conflict and recompute.
4. Use `execute` action `view.set-default` to choose the shared default; delete only with explicit authority and the exact saved `expected_etag` via `destroy` action `view.destroy`.

Views are shared data, not private preferences. Skills do not grant mutation permission. Source projects and dependency guidance remain MCP-only, and images never prove the desired revision has rendered.
