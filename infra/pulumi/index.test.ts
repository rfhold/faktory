import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, describe, test } from "node:test";
import * as pulumi from "@pulumi/pulumi";
import { requireImmutableImage, validateHttpsOrigin } from "./policy";

interface ResourceRecord { type: string; name: string; inputs: Record<string, any>; }
const resources: ResourceRecord[] = [];
const previousConfig = process.env.PULUMI_CONFIG;

before(async () => {
  process.env.PULUMI_CONFIG = JSON.stringify({
    "faktory:namespace": "faktory-test",
    "faktory:hostname": "faktory.example.test",
    "faktory:displayName": "Faktory Test",
    "faktory:slug": "faktory-test",
    "faktory:image": `registry.example.test/faktory@sha256:${"a".repeat(64)}`,
    "faktory:authentikBaseUrl": "https://auth.example.test",
    "faktory:s3Endpoint": "https://app-s3.example.test",
    "faktory:backupEndpoint": "https://backup-s3.example.test",
    "faktory:storageClass": "test-storage",
    "faktory:bucketStorageClass": "test-app-bucket",
    "faktory:backupStorageClass": "test-backup-bucket",
    "faktory:backupRetention": "7d",
    "faktory:databaseStorageSize": "2Gi",
    "faktory:protectData": "false",
  });
  pulumi.runtime.setMocks({
    newResource: (args) => {
      resources.push({ type: args.type, name: args.name, inputs: unwrap(args.inputs) });
      const state: Record<string, unknown> = { ...args.inputs };
      if (args.type === "random:index/randomBytes:RandomBytes") state.base64 = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
      if (args.type === "random:index/randomPassword:RandomPassword") state.result = "test-authentik-client-secret";
      if (args.type === "tls:index/privateKey:PrivateKey") state.privateKeyPem = "test-private-key";
      if (args.type === "tls:index/selfSignedCert:SelfSignedCert") state.certPem = "test-certificate";
      if (args.type === "authentik:index/certificateKeyPair:CertificateKeyPair") state.id = "signing-key";
      if (args.name === "faktory-artifacts-generated-config") state.data = { BUCKET_NAME: "test-artifacts" };
      if (args.name === "faktory-artifacts-generated-secret") state.data = {
        AWS_ACCESS_KEY_ID: Buffer.from("generated-access").toString("base64"),
        AWS_SECRET_ACCESS_KEY: Buffer.from("generated-secret").toString("base64"),
      };
      if (args.name === "faktory-backups-generated-config") state.data = { BUCKET_NAME: "test-backups" };
      if (args.name === "faktory-postgres-generated-app") state.data = {
        username: Buffer.from("app").toString("base64"),
        password: Buffer.from("password").toString("base64"),
      };
      return { id: `${args.name}-id`, state };
    },
    call: (args) => ({
      ...args.inputs,
      id: "lookup-id",
      propertyMappingProviderScopeId: "mapping-id",
      data: {
        BUCKET_NAME: args.inputs.name?.includes("backups") ? "test-backups" : "test-artifacts",
        AWS_ACCESS_KEY_ID: Buffer.from("generated-access").toString("base64"),
        AWS_SECRET_ACCESS_KEY: Buffer.from("generated-secret").toString("base64"),
        username: Buffer.from("app").toString("base64"),
        password: Buffer.from("password").toString("base64"),
      },
    }),
  }, "faktory", "test", false);
  await import("./index");
  await pulumi.runtime.disconnect();
});

after(() => {
  restore("PULUMI_CONFIG", previousConfig);
});

describe("configuration policy", () => {
  test("requires immutable images and credential-free HTTPS origins", () => {
    assert.equal(requireImmutableImage(`registry.test/faktory@sha256:${"b".repeat(64)}`).includes("@sha256:"), true);
    assert.throws(() => requireImmutableImage("registry.test/faktory:main"));
    assert.equal(validateHttpsOrigin("https://s3.example.test/", "s3"), "https://s3.example.test");
    assert.throws(() => validateHttpsOrigin("http://s3.example.test", "s3"));
  });

  test("defines secret-free preview and production stacks", () => {
    const project = readFileSync(join(process.cwd(), "Pulumi.yaml"), "utf8");
    const preview = stackFile("preview");
    const production = stackFile("prod");
    assert.doesNotMatch(project, /^\s*(?:s3Region|externalHttpsCidrs):/m);
    assert.match(preview, /^\s*faktory:namespace: faktory-preview$/m);
    assert.match(preview, /^\s*faktory:databaseStorageSize: 10Gi$/m);
    assert.match(preview, /^\s*faktory:backupRetention: 14d$/m);
    assert.match(preview, /^\s*faktory:protectData: (?:"false"|false)$/m);
    assert.match(production, /^\s*faktory:namespace: faktory$/m);
    assert.match(production, /^\s*faktory:hostname: faktory\.holdenitdown\.net$/m);
    assert.match(production, /^\s*faktory:databaseStorageSize: 20Gi$/m);
    assert.match(production, /^\s*faktory:backupRetention: 30d$/m);
    assert.match(production, /^\s*faktory:protectData: (?:"true"|true)$/m);
    for (const stack of [preview, production]) {
      assert.match(stack, /^\s*kubernetes:context: pantheon$/m);
      assert.match(stack, /^\s*faktory:s3Endpoint: https:\/\/s3\.pantheon\.holdenitdown\.net$/m);
      assert.match(stack, /^\s*faktory:backupEndpoint: https:\/\/s3\.pantheon\.holdenitdown\.net$/m);
      assert.match(stack, /^\s*faktory:bucketStorageClass: default-bucket$/m);
      assert.match(stack, /^\s*faktory:backupStorageClass: default-bucket$/m);
      assert.doesNotMatch(stack, /^\s*faktory:(?:s3Region|externalHttpsCidrs):/m);
      assert.doesNotMatch(stack, /^\s*faktory:image:/m);
      assert.doesNotMatch(stack, /(?:accessKey|secretKey|password|clientSecret)\s*:/i);
    }
    const program = readFileSync(join(process.cwd(), "index.ts"), "utf8");
    assert.equal((program.match(/protect: protectData/g) ?? []).length, 3);
    assert.doesNotMatch(program, /process\.env\.FAKTORY_S3_(?:ACCESS_KEY|SECRET_KEY)/);
  });
});

describe("preview declarations", () => {
  test("declares one hardened server and externalized runtime secrets", () => {
    const deployment = resource("kubernetes:apps/v1:Deployment", "faktory").inputs.spec;
    assert.equal(deployment.replicas, 1);
    assert.equal(deployment.strategy.type, "Recreate");
    const pod = deployment.template.spec;
    assert.equal(pod.automountServiceAccountToken, false);
    assert.equal(pod.nodeSelector, undefined);
    const container = pod.containers[0];
    assert.equal(container.securityContext.readOnlyRootFilesystem, true);
    assert.deepEqual(container.securityContext.capabilities.drop, ["ALL"]);
    assert.equal(container.startupProbe.httpGet.path, "/health");
    assert.equal(container.readinessProbe.httpGet.path, "/ready");
    const secret = resource("kubernetes:core/v1:Secret", "faktory-app").inputs.stringData;
    assert.equal(secret.FAKTORY_AUTH_MODE, "production");
    assert.equal(secret.FAKTORY_STATIC_DIR, "/opt/faktory/web");
    assert.equal(
      secret.FAKTORY_RENDER_COMMAND_JSON,
      '["/opt/faktory/env/bin/python","-m","renderer"]',
    );
    assert.equal(secret.FAKTORY_OAUTH_ACCESS_TOKEN_TTL_SECONDS, "900");
    assert.equal(secret.FAKTORY_OAUTH_REFRESH_FAMILY_TTL_SECONDS, "2592000");
    assert.equal(secret.FAKTORY_OAUTH_WRAPPING_KEYS_FILE, "/var/run/secrets/faktory/oauth/keyring.json");
    assert.equal(secret.FAKTORY_OAUTH_ALLOW_DCR, "true");
    assert.equal(secret.FAKTORY_OAUTH_ALLOW_LOOPBACK_REDIRECTS, "true");
    assert.equal(secret.FAKTORY_S3_BUCKET, "test-artifacts");
    assert.equal(secret.FAKTORY_S3_ENDPOINT, "https://app-s3.example.test");
    assert.equal(secret.FAKTORY_S3_ACCESS_KEY, "generated-access");
    assert.equal(secret.FAKTORY_S3_SECRET_KEY, "generated-secret");
    assert.equal(secret.FAKTORY_S3_REGION, "us-east-1");
  });

  test("declares separate artifact and backup buckets with CNPG backups", () => {
    const claims = resources.filter((candidate) => candidate.inputs.kind === "ObjectBucketClaim");
    assert.equal(claims.length, 2);
    assert.deepEqual(resourceByName("faktory-artifacts-bucket").inputs.spec, {
      storageClassName: "test-app-bucket",
      generateBucketName: "faktory-test-artifacts",
    });
    assert.equal(resourceByName("faktory-artifacts-bucket").inputs.metadata.name, "faktory-artifacts");
    assert.deepEqual(resourceByName("faktory-backups-bucket").inputs.spec, {
      storageClassName: "test-backup-bucket",
      generateBucketName: "faktory-test-backups",
    });
    assert.equal(resourceByName("faktory-backups-bucket").inputs.metadata.name, "faktory-backups");

    const cluster = resourceByName("faktory-postgres").inputs;
    assert.equal(cluster.kind, "Cluster");
    assert.equal(cluster.spec.backup.retentionPolicy, "7d");
    assert.equal(cluster.spec.backup.barmanObjectStore.destinationPath, "s3://test-backups/database");
    assert.equal(cluster.spec.backup.barmanObjectStore.endpointURL, "https://backup-s3.example.test");
    assert.deepEqual(cluster.spec.backup.barmanObjectStore.s3Credentials, {
      accessKeyId: { name: "faktory-backups", key: "AWS_ACCESS_KEY_ID" },
      secretAccessKey: { name: "faktory-backups", key: "AWS_SECRET_ACCESS_KEY" },
    });
    assert.deepEqual(cluster.spec.backup.barmanObjectStore.wal, { compression: "gzip" });
    assert.deepEqual(cluster.spec.backup.barmanObjectStore.data, { compression: "gzip" });
    assert.deepEqual(resourceByName("faktory-postgres-backup").inputs.spec, {
      schedule: "0 0 2 * * *",
      backupOwnerReference: "self",
      cluster: { name: "faktory-postgres" },
      immediate: true,
      method: "barmanObjectStore",
    });
  });

  test("declares PostgreSQL, Authentik, routing, and network policy", () => {
    assert.equal(resourceByName("faktory-database").inputs.kind, "Database");
    const provider = resources.find((candidate) => candidate.type === "authentik:index/providerOauth2:ProviderOauth2");
    assert.equal(provider?.inputs.clientType, "confidential");
    assert.deepEqual(provider?.inputs.allowedRedirectUris, [
      { matching_mode: "strict", url: "https://faktory.example.test/oidc/callback" },
      { matching_mode: "strict", url: "https://faktory.example.test/oauth/oidc/callback" },
    ]);
    assert.equal(resourceByName("faktory-route").inputs.kind, "HTTPRoute");
    const network = resource("kubernetes:networking.k8s.io/v1:NetworkPolicy", "faktory-network").inputs.spec;
    assert.deepEqual(network.policyTypes, ["Ingress", "Egress"]);
    assert.deepEqual(network.ingress, [{
      from: [{ namespaceSelector: { matchLabels: { "kubernetes.io/metadata.name": "ingress" } } }],
      ports: [{ port: 8080, protocol: "TCP" }],
    }]);
    assert.deepEqual(network.egress, [
      {
        to: [{ namespaceSelector: { matchLabels: { "kubernetes.io/metadata.name": "kube-system" } } }],
        ports: [{ port: 53, protocol: "UDP" }, { port: 53, protocol: "TCP" }],
      },
      {
        ports: [
          { port: 443, protocol: "TCP" },
          { port: 4040, protocol: "TCP" },
          { port: 4318, protocol: "TCP" },
        ],
      },
      {
        to: [{ podSelector: { matchLabels: { "cnpg.io/cluster": "faktory-postgres" } } }],
        ports: [{ port: 5432, protocol: "TCP" }],
      },
    ]);
  });

  test("wires stack-aware telemetry and Kubernetes identity metadata", () => {
    const secret = resource("kubernetes:core/v1:Secret", "faktory-app").inputs.stringData;
    assert.equal(secret.FAKTORY_DEPLOYMENT_ENVIRONMENT, "test");
    assert.equal(secret.FAKTORY_PYROSCOPE_URL, "https://telemetry.holdenitdown.net:4040");
    assert.equal(secret.OTEL_EXPORTER_OTLP_ENDPOINT, "https://telemetry.holdenitdown.net:4318");
    assert.equal(secret.OTEL_EXPORTER_OTLP_PROTOCOL, "http/protobuf");
    assert.equal(secret.OTEL_SERVICE_NAME, "faktory");
    assert.equal(secret.OTEL_RESOURCE_ATTRIBUTES, "service.namespace=faktory,deployment.environment.name=test");

    const deployment = resource("kubernetes:apps/v1:Deployment", "faktory").inputs.spec;
    assert.deepEqual(deployment.template.metadata.annotations, {
      "faktory.holdenitdown.net/wrapping-key-checksum": deployment.template.metadata.annotations["faktory.holdenitdown.net/wrapping-key-checksum"],
      "resource.opentelemetry.io/service.name": "faktory",
      "resource.opentelemetry.io/service.namespace": "faktory",
      "resource.opentelemetry.io/deployment.environment.name": "test",
    });
    assert.deepEqual(deployment.template.spec.containers[0].env, [
      { name: "FAKTORY_K8S_NAMESPACE", valueFrom: { fieldRef: { fieldPath: "metadata.namespace" } } },
      { name: "FAKTORY_K8S_POD_NAME", valueFrom: { fieldRef: { fieldPath: "metadata.name" } } },
      { name: "FAKTORY_K8S_POD_UID", valueFrom: { fieldRef: { fieldPath: "metadata.uid" } } },
    ]);
  });
});

function resource(type: string, name: string): ResourceRecord {
  const match = resources.find((candidate) => candidate.type === type && candidate.name === name);
  assert.ok(match, `missing ${type} ${name}`);
  return match;
}

function resourceByName(name: string): ResourceRecord {
  const match = resources.find((candidate) => candidate.name === name);
  assert.ok(match, `missing ${name}`);
  return match;
}

function restore(name: string, value: string | undefined): void {
  if (value === undefined) delete process.env[name];
  else process.env[name] = value;
}

function stackFile(stack: string): string {
  return readFileSync(join(process.cwd(), `Pulumi.${stack}.yaml`), "utf8");
}

function unwrap(value: any): any {
  if (Array.isArray(value)) return value.map(unwrap);
  if (value && typeof value === "object") {
    if ("4dabf18193072939515e22adb298388d" in value && "value" in value) return unwrap(value.value);
    return Object.fromEntries(Object.entries(value).map(([key, nested]) => [key, unwrap(nested)]));
  }
  return value;
}
