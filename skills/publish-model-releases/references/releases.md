# Release Tools and Compatibility

`execute` accepts exactly `{action, input}`; publication arguments belong in `input`. No filter or flat argument object is accepted.

```json
{"action":"model.release.publish","input":{"model_id":"part","version":"1.0.0","expected_revision":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}
```

| Tool | Input and behavior |
| --- | --- |
| Model `/releases` resource | Immutable stable versions and exact release URI links, no source bodies. |
| Model `/releases/{version}` resource | Release digest, project revision, namespace, exact closure identities, file hashes and immutable file links, no bodies. |
| `execute` action `model.release.publish` | `model_id`, stable `version`, `expected_revision`; permanently publishes an exact READY desired/current-successful project containing `faktory_model/__init__.py`. |

All inputs reject unknown fields. IDs are kebab-case ASCII at most 64 bytes. Revisions are lowercase SHA-256 (64 characters). Versions are exactly `MAJOR.MINOR.PATCH`: no leading zeros, prerelease, or build suffix. A new version must exceed every published version. Byte-identical republication is idempotent; same version with different canonical bytes conflicts. Releases cannot be deleted, yanked, moved, or edited. Rendered bytes are not release identity.

Read `faktory://models/{model_id}/revisions/{project_revision}/files/{path}` for released source; follow canonical URI links rather than interpolating unescaped paths. There is no mutable release default. Initial requirements and explicit `execute` action `dependencies.set` choose highest compatible published releases; renders and retry use stored exact locks without consulting the mutable catalog.

Minor/patch publication rolls eligible same-major direct consumers, validates their exact closure, regenerates their project AGENTS.md, and creates a deterministic PENDING revision. Rollout is durable and restart-resumable, preserves last-good artifacts, and reports updated/already-current/not-eligible counts independently of asynchronous render success. A new major never changes a consumer automatically. Explicit guarded `dependencies.set` with the new same-major range adopts it. Authors, not Faktory, certify semantic and physical compatibility.
