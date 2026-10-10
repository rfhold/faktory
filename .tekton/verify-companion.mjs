import assert from "node:assert/strict";

const origin = "http://127.0.0.1:8082";
let response;
for (let attempt = 0; attempt < 60; attempt++) {
  try {
    response = await fetch(origin + "/frame");
    if (response.ok) break;
  } catch {}
  await new Promise((resolve) => setTimeout(resolve, 250));
}
assert.equal(response?.status, 200);
assert.match(response.headers.get("content-type"), /text\/html/);
assert.equal(response.headers.get("set-cookie"), null);
const policy = response.headers.get("content-security-policy");
assert.ok(policy);
assert.match(policy, /frame-ancestors https:\/\/app\.example\.test(?:;|$)/);
assert.doesNotMatch(policy, /unsafe-eval|https:\/\/\*/);
for (const path of ["/oidc/callback", "/api/models", "/.env", "/../package.json"]) {
  const denied = await fetch(origin + path);
  assert.equal(denied.status, 404, path);
  assert.equal(denied.headers.get("set-cookie"), null);
}
