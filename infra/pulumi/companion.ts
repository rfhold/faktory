import * as k8s from "@pulumi/kubernetes";
import * as pulumi from "@pulumi/pulumi";

export function declareCompanionHost(
  config: { hostname: string; image: string },
  namespace: pulumi.Input<string>,
  appLabels: Record<string, string>,
  parentOrigin: string,
) {
  const labels = { ...appLabels, "app.kubernetes.io/component": "companion-host" };
  const service = new k8s.core.v1.Service("faktory-companion-host", {
    metadata: { name: "companion-host", namespace, labels },
    spec: { type: "ClusterIP", selector: labels, ports: [{ name: "http", port: 8082, targetPort: "http" }] },
  });
  new k8s.apps.v1.Deployment("faktory-companion-host", {
    metadata: { name: "companion-host", namespace, labels },
    spec: {
      replicas: 1,
      selector: { matchLabels: labels },
      template: {
        metadata: { labels },
        spec: {
          automountServiceAccountToken: false,
          securityContext: { runAsNonRoot: true, runAsUser: 65532, runAsGroup: 65532, seccompProfile: { type: "RuntimeDefault" } },
          containers: [{
            name: "companion-host", image: config.image,
            ports: [{ name: "http", containerPort: 8082 }],
            env: [{ name: "PORT", value: "8082" }, { name: "COMPANION_PARENT_ORIGINS", value: parentOrigin }],
            securityContext: { allowPrivilegeEscalation: false, readOnlyRootFilesystem: true, capabilities: { drop: ["ALL"] } },
            resources: { requests: { cpu: "25m", memory: "64Mi" }, limits: { cpu: "250m", memory: "128Mi" } },
            readinessProbe: { httpGet: { path: "/ready", port: "http" }, periodSeconds: 5 },
            livenessProbe: { httpGet: { path: "/health", port: "http" }, periodSeconds: 10 },
          }],
        },
      },
    },
  });
  new k8s.networking.v1.NetworkPolicy("faktory-companion-host-network", {
    metadata: { name: "companion-host", namespace, labels },
    spec: {
      podSelector: { matchLabels: labels }, policyTypes: ["Ingress", "Egress"], egress: [],
      ingress: [{ from: [{ namespaceSelector: { matchLabels: { "kubernetes.io/metadata.name": "ingress" } } }], ports: [{ port: 8082, protocol: "TCP" }] }],
    },
  });
  new k8s.apiextensions.CustomResource("faktory-companion-host-route", {
    apiVersion: "gateway.networking.k8s.io/v1", kind: "HTTPRoute",
    metadata: { name: "companion-host", namespace, labels },
    spec: {
      parentRefs: [{ group: "gateway.networking.k8s.io", kind: "Gateway", name: "default-gateway", namespace: "ingress" }],
      hostnames: [config.hostname],
      rules: [{ matches: [{ path: { type: "PathPrefix", value: "/" } }], backendRefs: [{ name: service.metadata.name, port: 8082 }] }],
    },
  });
}
