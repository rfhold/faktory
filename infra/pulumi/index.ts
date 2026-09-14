import { createHash } from "node:crypto";
import * as authentik from "@pulumi/authentik";
import * as k8s from "@pulumi/kubernetes";
import * as pulumi from "@pulumi/pulumi";
import * as random from "@pulumi/random";
import * as tls from "@pulumi/tls";
import { BrowserApplication } from "./authentik";
import { requireImmutableImage, validateHostname, validateHttpsOrigin, validateSegment } from "./policy";

const config = new pulumi.Config();
const namespaceName = validateSegment(config.require("namespace"), "namespace");
const hostname = validateHostname(config.require("hostname"));
const displayName = config.require("displayName");
const slug = validateSegment(config.require("slug"), "slug");
const image = requireImmutableImage(config.require("image"));
const authentikBaseUrl = validateHttpsOrigin(config.require("authentikBaseUrl"), "authentikBaseUrl");
const s3Endpoint = validateHttpsOrigin(config.require("s3Endpoint"), "s3Endpoint");
const backupEndpoint = validateHttpsOrigin(config.require("backupEndpoint"), "backupEndpoint");
const storageClass = config.require("storageClass");
const bucketStorageClass = config.require("bucketStorageClass");
const backupStorageClass = config.require("backupStorageClass");
const backupRetention = config.require("backupRetention");
const databaseStorageSize = config.require("databaseStorageSize");
const protectData = config.requireBoolean("protectData");
const oauthWrappingKeyVersions = config.requireObject<unknown>("oauthWrappingKeyVersions");
const oauthActiveWrappingKeyVersion = config.require("oauthActiveWrappingKeyVersion");
validateOAuthWrappingKeyConfiguration(oauthWrappingKeyVersions, oauthActiveWrappingKeyVersion);
const deploymentEnvironment = pulumi.getStack();
const publicUrl = `https://${hostname}`;
const labels = {
  "app.kubernetes.io/name": "faktory",
  "app.kubernetes.io/instance": slug,
  "app.kubernetes.io/managed-by": "pulumi",
};
const workloadLabels = { ...labels, "app.kubernetes.io/component": "server" };

const namespace = new k8s.core.v1.Namespace("faktory-namespace", {
  metadata: { name: namespaceName, labels },
});

const artifactsBucket = new k8s.apiextensions.CustomResource("faktory-artifacts-bucket", {
  apiVersion: "objectbucket.io/v1alpha1",
  kind: "ObjectBucketClaim",
  metadata: { name: "faktory-artifacts", namespace: namespace.metadata.name, labels },
  spec: {
    storageClassName: bucketStorageClass,
    generateBucketName: `${slug}-artifacts`,
  },
}, { dependsOn: [namespace], protect: protectData });
const artifactsConfig = pulumi.all([namespace.metadata.name, artifactsBucket.id]).apply(([resolvedNamespace]) =>
  k8s.core.v1.ConfigMap.get("faktory-artifacts-generated-config", `${resolvedNamespace}/faktory-artifacts`),
);
const artifactsSecret = pulumi.all([namespace.metadata.name, artifactsBucket.id]).apply(([resolvedNamespace]) =>
  k8s.core.v1.Secret.get("faktory-artifacts-generated-secret", `${resolvedNamespace}/faktory-artifacts`),
);

const backupBucket = new k8s.apiextensions.CustomResource("faktory-backups-bucket", {
  apiVersion: "objectbucket.io/v1alpha1",
  kind: "ObjectBucketClaim",
  metadata: { name: "faktory-backups", namespace: namespace.metadata.name, labels },
  spec: {
    storageClassName: backupStorageClass,
    generateBucketName: `${slug}-backups`,
  },
}, { dependsOn: [namespace], protect: protectData });
const backupConfig = pulumi.all([namespace.metadata.name, backupBucket.id]).apply(([resolvedNamespace]) =>
  k8s.core.v1.ConfigMap.get("faktory-backups-generated-config", `${resolvedNamespace}/faktory-backups`),
);

const databaseCluster = new k8s.apiextensions.CustomResource("faktory-postgres", {
  apiVersion: "postgresql.cnpg.io/v1",
  kind: "Cluster",
  metadata: { name: "faktory-postgres", namespace: namespace.metadata.name, labels },
  spec: {
    instances: 1,
    enableSuperuserAccess: false,
    backup: {
      retentionPolicy: backupRetention,
      barmanObjectStore: {
        destinationPath: backupConfig.data.apply((values) => `s3://${values.BUCKET_NAME}/database`),
        endpointURL: backupEndpoint,
        s3Credentials: {
          accessKeyId: { name: "faktory-backups", key: "AWS_ACCESS_KEY_ID" },
          secretAccessKey: { name: "faktory-backups", key: "AWS_SECRET_ACCESS_KEY" },
        },
        wal: { compression: "gzip" },
        data: { compression: "gzip" },
      },
    },
    storage: { size: databaseStorageSize, storageClass },
    resources: {
      requests: { cpu: "100m", memory: "256Mi" },
      limits: { cpu: "1", memory: "1Gi" },
    },
  },
}, { dependsOn: [backupBucket], protect: protectData });

new k8s.apiextensions.CustomResource("faktory-postgres-backup", {
  apiVersion: "postgresql.cnpg.io/v1",
  kind: "ScheduledBackup",
  metadata: { name: "faktory-postgres", namespace: namespace.metadata.name, labels },
  spec: {
    schedule: "0 0 2 * * *",
    backupOwnerReference: "self",
    cluster: { name: "faktory-postgres" },
    immediate: true,
    method: "barmanObjectStore",
  },
}, { dependsOn: [databaseCluster] });

const database = new k8s.apiextensions.CustomResource("faktory-database", {
  apiVersion: "postgresql.cnpg.io/v1",
  kind: "Database",
  metadata: { name: "faktory", namespace: namespace.metadata.name, labels },
  spec: { name: "faktory", owner: "app", cluster: { name: "faktory-postgres" } },
}, { dependsOn: [databaseCluster] });

const signingPrivateKey = new tls.PrivateKey("faktory-oidc-signing-key", { algorithm: "RSA", rsaBits: 4096 });
const signingCertificate = new tls.SelfSignedCert("faktory-oidc-signing-certificate", {
  privateKeyPem: signingPrivateKey.privateKeyPem,
  subject: { commonName: `${slug} OIDC signing` },
  validityPeriodHours: 87600,
  allowedUses: ["digital_signature", "key_encipherment"],
});
const signingKey = new authentik.CertificateKeyPair("faktory-oidc-signing-keypair", {
  name: `${displayName} OIDC signing key`,
  certificateData: signingCertificate.certPem,
  keyData: signingPrivateKey.privateKeyPem,
});
const scopeMappings = ["openid", "profile", "email"].map((scope) =>
  authentik.getPropertyMappingProviderScopeOutput({ managed: `goauthentik.io/providers/oauth2/scope-${scope}` }),
);
const browserApp = new BrowserApplication("faktory-browser", {
  displayName,
  slug,
  issuerBaseUrl: authentikBaseUrl,
  redirectUris: [
    `${publicUrl}/oidc/callback`,
    `${publicUrl}/oauth/oidc/callback`,
  ],
  launchUrl: publicUrl,
  signingKeyId: signingKey.id,
  propertyMappings: scopeMappings.map((mapping) => mapping.id),
});

const cnpgAppSecret = pulumi.all([namespace.metadata.name, databaseCluster.id]).apply(([resolvedNamespace]) =>
  k8s.core.v1.Secret.get("faktory-postgres-generated-app", `${resolvedNamespace}/faktory-postgres-app`),
);
const decodeDatabaseSecret = (key: string) => cnpgAppSecret.data.apply((values) => Buffer.from(values[key], "base64").toString("utf8"));
const decodeArtifactsSecret = (key: string) => artifactsSecret.data.apply((values) => Buffer.from(values[key], "base64").toString("utf8"));
const databaseUrl = pulumi.all([decodeDatabaseSecret("username"), decodeDatabaseSecret("password")]).apply(
  ([username, password]) => `postgresql://${encodeURIComponent(username)}:${encodeURIComponent(password)}@faktory-postgres-rw.${namespaceName}.svc:5432/faktory?sslmode=verify-full&sslrootcert=%2Fvar%2Frun%2Fsecrets%2Ffaktory%2Fpostgres%2Fca.crt`,
);

const wrappingKeys = oauthWrappingKeyVersions.map((version) => new random.RandomBytes(
  `faktory-oauth-wrapping-key-${version}`,
  { length: 32 },
  { protect: protectData },
).base64);
const wrappingKeyring = pulumi.secret(pulumi.all(wrappingKeys).apply((keys) => JSON.stringify({
  schema_version: 1,
  active: oauthActiveWrappingKeyVersion,
  keys: oauthWrappingKeyVersions.map((version, index) => ({
    id: version,
    key: Buffer.from(keys[index], "base64").toString("base64url"),
  })),
})));
const wrappingKeyChecksum = wrappingKeyring.apply((value) => createHash("sha256").update(value).digest("hex"));
const wrappingKeySecret = new k8s.core.v1.Secret("faktory-oauth-wrapping-keys", {
  metadata: { name: "faktory-oauth-wrapping-keys", namespace: namespace.metadata.name, labels },
  type: "Opaque",
  stringData: { "keyring.json": wrappingKeyring },
});

const appSecret = new k8s.core.v1.Secret("faktory-app", {
  metadata: { name: "faktory-app", namespace: namespace.metadata.name, labels },
  type: "Opaque",
  stringData: {
    FAKTORY_AUTH_MODE: "production",
    FAKTORY_DATABASE_URL: databaseUrl,
    FAKTORY_PUBLIC_BASE_URL: publicUrl,
    FAKTORY_OIDC_ISSUER: browserApp.issuer,
    FAKTORY_OIDC_CLIENT_ID: browserApp.clientId,
    FAKTORY_OIDC_CLIENT_SECRET: browserApp.clientSecret,
    FAKTORY_SESSION_TTL_SECONDS: "28800",
    FAKTORY_OAUTH_ACCESS_TOKEN_TTL_SECONDS: "900",
    FAKTORY_OAUTH_REFRESH_TOKEN_TTL_SECONDS: "86400",
    FAKTORY_OAUTH_REFRESH_FAMILY_TTL_SECONDS: "2592000",
    FAKTORY_OAUTH_CODE_TTL_SECONDS: "300",
    FAKTORY_OAUTH_WRAPPING_KEYS_FILE: "/var/run/secrets/faktory/oauth/keyring.json",
    FAKTORY_OAUTH_ALLOW_DCR: "true",
    FAKTORY_OAUTH_ALLOW_CIMD: "true",
    FAKTORY_OAUTH_ALLOW_LOOPBACK_REDIRECTS: "true",
    FAKTORY_DEPLOYMENT_ENVIRONMENT: deploymentEnvironment,
    FAKTORY_PYROSCOPE_URL: "https://telemetry.holdenitdown.net:4040",
    OTEL_EXPORTER_OTLP_ENDPOINT: "https://telemetry.holdenitdown.net:4318",
    OTEL_EXPORTER_OTLP_PROTOCOL: "http/protobuf",
    OTEL_SERVICE_NAME: "faktory",
    OTEL_RESOURCE_ATTRIBUTES: `service.namespace=faktory,deployment.environment.name=${deploymentEnvironment}`,
    FAKTORY_S3_ENDPOINT: s3Endpoint,
    FAKTORY_S3_REGION: "us-east-1",
    FAKTORY_S3_BUCKET: artifactsConfig.data.apply((values) => values.BUCKET_NAME),
    FAKTORY_S3_ACCESS_KEY: decodeArtifactsSecret("AWS_ACCESS_KEY_ID"),
    FAKTORY_S3_SECRET_KEY: decodeArtifactsSecret("AWS_SECRET_ACCESS_KEY"),
    FAKTORY_RENDER_COMMAND_JSON: '["/opt/faktory/env/bin/python","-m","renderer"]',
    FAKTORY_STATIC_DIR: "/opt/faktory/web",
  },
}, { dependsOn: [database, browserApp, artifactsBucket] });

new k8s.apps.v1.Deployment("faktory", {
  metadata: {
    name: "faktory",
    namespace: namespace.metadata.name,
    labels: workloadLabels,
    annotations: { "secret.reloader.stakater.com/reload": "faktory-app,faktory-oauth-wrapping-keys" },
  },
  spec: {
    replicas: 1,
    strategy: { type: "Recreate" },
    selector: { matchLabels: workloadLabels },
    template: {
      metadata: {
        labels: workloadLabels,
        annotations: {
          "faktory.holdenitdown.net/wrapping-key-checksum": wrappingKeyChecksum,
          "resource.opentelemetry.io/service.name": "faktory",
          "resource.opentelemetry.io/service.namespace": "faktory",
          "resource.opentelemetry.io/deployment.environment.name": deploymentEnvironment,
        },
      },
      spec: {
        automountServiceAccountToken: false,
        securityContext: { runAsNonRoot: true, runAsUser: 65532, runAsGroup: 65532, fsGroup: 65532, seccompProfile: { type: "RuntimeDefault" } },
        terminationGracePeriodSeconds: 30,
        containers: [{
          name: "faktory",
          image,
          imagePullPolicy: "IfNotPresent",
          ports: [{ name: "http", containerPort: 8080, protocol: "TCP" }],
          envFrom: [{ secretRef: { name: appSecret.metadata.name } }],
          env: [
            { name: "FAKTORY_K8S_NAMESPACE", valueFrom: { fieldRef: { fieldPath: "metadata.namespace" } } },
            { name: "FAKTORY_K8S_POD_NAME", valueFrom: { fieldRef: { fieldPath: "metadata.name" } } },
            { name: "FAKTORY_K8S_POD_UID", valueFrom: { fieldRef: { fieldPath: "metadata.uid" } } },
          ],
          securityContext: { runAsNonRoot: true, allowPrivilegeEscalation: false, readOnlyRootFilesystem: true, capabilities: { drop: ["ALL"] } },
          resources: { requests: { cpu: "100m", memory: "512Mi" }, limits: { cpu: "2", memory: "2Gi" } },
          startupProbe: { httpGet: { path: "/health", port: "http" }, periodSeconds: 2, failureThreshold: 60 },
          readinessProbe: { httpGet: { path: "/ready", port: "http" }, periodSeconds: 5, failureThreshold: 3 },
          livenessProbe: { httpGet: { path: "/health", port: "http" }, periodSeconds: 10, failureThreshold: 3 },
          volumeMounts: [
            { name: "tmp", mountPath: "/tmp" },
            { name: "oauth", mountPath: "/var/run/secrets/faktory/oauth", readOnly: true },
            { name: "postgres-ca", mountPath: "/var/run/secrets/faktory/postgres", readOnly: true },
          ],
        }],
        volumes: [
          { name: "tmp", emptyDir: { sizeLimit: "256Mi" } },
          { name: "oauth", secret: { secretName: wrappingKeySecret.metadata.name, defaultMode: 0o440 } },
          { name: "postgres-ca", secret: { secretName: "faktory-postgres-ca", defaultMode: 0o444, items: [{ key: "ca.crt", path: "ca.crt" }] } },
        ],
      },
    },
  },
}, { dependsOn: [appSecret, wrappingKeySecret] });

new k8s.networking.v1.NetworkPolicy("faktory-network", {
  metadata: { name: "faktory", namespace: namespace.metadata.name, labels },
  spec: {
    podSelector: { matchLabels: workloadLabels },
    policyTypes: ["Ingress", "Egress"],
    ingress: [{
      from: [{ namespaceSelector: { matchLabels: { "kubernetes.io/metadata.name": "ingress" } } }],
      ports: [{ port: 8080, protocol: "TCP" }],
    }],
    egress: [
      { to: [{ namespaceSelector: { matchLabels: { "kubernetes.io/metadata.name": "kube-system" } } }], ports: [{ port: 53, protocol: "UDP" }, { port: 53, protocol: "TCP" }] },
      { ports: [{ port: 443, protocol: "TCP" }, { port: 4040, protocol: "TCP" }, { port: 4318, protocol: "TCP" }] },
      { to: [{ podSelector: { matchLabels: { "cnpg.io/cluster": "faktory-postgres" } } }], ports: [{ port: 5432, protocol: "TCP" }] },
    ],
  },
});

const service = new k8s.core.v1.Service("faktory", {
  metadata: { name: "faktory", namespace: namespace.metadata.name, labels },
  spec: { type: "ClusterIP", selector: workloadLabels, ports: [{ name: "http", port: 8080, targetPort: "http" }] },
});

new k8s.apiextensions.CustomResource("faktory-route", {
  apiVersion: "gateway.networking.k8s.io/v1",
  kind: "HTTPRoute",
  metadata: { name: "faktory", namespace: namespace.metadata.name, labels },
  spec: {
    parentRefs: [{ group: "gateway.networking.k8s.io", kind: "Gateway", name: "default-gateway", namespace: "ingress" }],
    hostnames: [hostname],
    rules: [{ matches: [{ path: { type: "PathPrefix", value: "/" } }], backendRefs: [{ name: service.metadata.name, port: 8080 }], timeouts: { request: "0s" } }],
  },
});

export const deployedNamespace = namespace.metadata.name;
export const deployedUrl = publicUrl;

export function validateOAuthWrappingKeyConfiguration(versions: unknown, activeVersion: string): asserts versions is string[] {
  const dnsLabel = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
  if (
    !Array.isArray(versions) ||
    versions.length === 0 ||
    versions.length > 32 ||
    versions.some((version) => typeof version !== "string" || !dnsLabel.test(version)) ||
    new Set(versions).size !== versions.length ||
    !versions.includes(activeVersion)
  ) {
    throw new Error(
      "oauthWrappingKeyVersions must contain 1 to 32 unique DNS labels of at most 63 characters and include oauthActiveWrappingKeyVersion",
    );
  }
}
