import assert from "node:assert/strict";
import { test } from "node:test";
import * as pulumi from "@pulumi/pulumi";
import { declareCompanionHost } from "./companion";

test("enabled companion has exact origins, separate route, no secrets or egress", async () => {
  const resources: { name: string; inputs: any }[] = [];
  pulumi.runtime.setMocks({
    newResource(args) {
      resources.push({ name: args.name, inputs: args.inputs });
      return { id: args.name, state: args.inputs };
    },
    call: (args) => args.inputs,
  }, "faktory", "test", false);
  declareCompanionHost({ hostname: "companion.example.test", image: `registry.test/companion@sha256:${"c".repeat(64)}` }, "test", { app: "faktory" }, "https://app.example.test");
  await pulumi.runtime.disconnect();
  const get = (name: string) => resources.find((resource) => resource.name === name)?.inputs;
  const deployment = resources.find(({ inputs }) => inputs.spec?.template)?.inputs;
  const pod = deployment.spec.template.spec;
  const container = pod.containers[0];
  assert.deepEqual(container.env, [{ name: "PORT", value: "8082" }, { name: "COMPANION_PARENT_ORIGINS", value: "https://app.example.test" }]);
  assert.equal(container.envFrom, undefined);
  assert.equal(pod.volumes, undefined);
  assert.equal(pod.automountServiceAccountToken, false);
  assert.equal(pod.securityContext.runAsUser, 65532);
  assert.equal(container.securityContext.readOnlyRootFilesystem, true);
  assert.deepEqual(container.securityContext.capabilities.drop, ["ALL"]);
  assert.deepEqual(get("faktory-companion-host-network").spec.egress, []);
  assert.deepEqual(get("faktory-companion-host-route").spec.hostnames, ["companion.example.test"]);
  assert.deepEqual(get("faktory-companion-host-route").spec.rules[0].backendRefs, [{ name: "companion-host", port: 8082 }]);
  assert.doesNotMatch(JSON.stringify(resources), /secretRef|secretKeyRef|FAKTORY_S3|FAKTORY_DATABASE|OIDC/);
});
