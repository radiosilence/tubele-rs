import * as k8s from "@pulumi/kubernetes";
import * as pulumi from "@pulumi/pulumi";
import { VERSIONS } from "./versions.ts";

/**
 * A hostname the deployment should publish, and the workload answering it.
 * `service` is the name prefix: the Service is `<prefix>-service`.
 */
export type Route = { service: string; hostname: string };

export type Deployed = { routes: Route[] };

const NAME = "tubele";
const PORT = 8080;
/** The uid the image runs as. */
const UID = 65532;

/**
 * tubele: the game, its boards and every player's record. State is one
 * SQLite file on the node's own disk at `statePath`, so the pod is pinned to
 * that node and runs one replica.
 */
export function createTubele(
  provider: k8s.Provider,
  namespace: pulumi.Input<string>,
  opts: {
    hostname: string;
    /** The node whose disk holds the database, by hostname. */
    node: string;
    /** Where on that node the database lives. */
    statePath?: string;
    limits?: { cpu: string; memory: string };
    requests?: { cpu: string; memory: string };
  },
) {
  const options = { provider };
  const labels = { app: NAME };
  const state = opts.statePath ?? "/var/lib/tubele";

  const deployment = new k8s.apps.v1.Deployment(
    NAME,
    {
      metadata: { name: NAME, namespace, labels },
      spec: {
        replicas: 1,
        // One SQLite file on one node: a surge would put two writers on it.
        strategy: { type: "Recreate" },
        selector: { matchLabels: labels },
        template: {
          metadata: { labels },
          spec: {
            nodeSelector: { "kubernetes.io/hostname": opts.node },
            automountServiceAccountToken: false,
            enableServiceLinks: false,
            securityContext: {
              runAsNonRoot: true,
              runAsUser: UID,
              runAsGroup: UID,
              seccompProfile: { type: "RuntimeDefault" },
            },
            // kubelet creates a missing hostPath as root, which the service
            // cannot write; this hands it over, with only the ownership
            // capabilities.
            initContainers: [
              {
                name: "state",
                image: VERSIONS.alpine,
                command: ["sh", "-c", `chown ${UID}:${UID} /data && chmod 700 /data`],
                securityContext: {
                  runAsUser: 0,
                  runAsNonRoot: false,
                  allowPrivilegeEscalation: false,
                  capabilities: { drop: ["ALL"], add: ["CHOWN", "FOWNER"] },
                },
                resources: { requests: { cpu: "5m", memory: "8Mi" }, limits: { memory: "32Mi" } },
                volumeMounts: [{ name: "state", mountPath: "/data" }],
              },
            ],
            containers: [
              {
                name: NAME,
                image: VERSIONS.tubele,
                imagePullPolicy: "IfNotPresent",
                ports: [{ name: "http", containerPort: PORT }],
                env: [
                  { name: "BIND_ADDR", value: `0.0.0.0:${PORT}` },
                  { name: "DATABASE_PATH", value: "/data/tubele.db" },
                  { name: "SITE_URL", value: `https://${opts.hostname}` },
                ],
                readinessProbe: { httpGet: { path: "/healthz", port: "http" }, periodSeconds: 10 },
                livenessProbe: {
                  httpGet: { path: "/healthz", port: "http" },
                  initialDelaySeconds: 5,
                  periodSeconds: 30,
                },
                resources: {
                  limits: opts.limits ?? { cpu: "500m", memory: "128Mi" },
                  requests: opts.requests,
                },
                securityContext: {
                  allowPrivilegeEscalation: false,
                  readOnlyRootFilesystem: true,
                  capabilities: { drop: ["ALL"] },
                },
                volumeMounts: [{ name: "state", mountPath: "/data" }],
              },
            ],
            volumes: [{ name: "state", hostPath: { path: state, type: "DirectoryOrCreate" } }],
          },
        },
      },
    },
    { ...options, deleteBeforeReplace: true },
  );

  // Named for what the ingress derives from the route prefix.
  new k8s.core.v1.Service(
    "tubele-service",
    {
      metadata: { name: "tubele-service", namespace },
      spec: { selector: labels, ports: [{ port: 80, targetPort: PORT }] },
    },
    options,
  );

  // The service calls nothing, so it may reach nothing. No ingress rule:
  // Traefik reaches it, and an ingress policy that also drops the kubelet's
  // probes gets the pod killed for failing readiness.
  new k8s.networking.v1.NetworkPolicy(
    "tubele-netpol",
    {
      metadata: { name: NAME, namespace },
      spec: {
        podSelector: { matchLabels: labels },
        policyTypes: ["Egress"],
        egress: [],
      },
    },
    options,
  );

  return {
    routes: [{ service: NAME, hostname: opts.hostname }],
    deployment,
  } satisfies Deployed & Record<string, unknown>;
}
