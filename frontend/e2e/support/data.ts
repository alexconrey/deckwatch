import type {
  AgentFeedbackItem,
  ApplicationDetail,
  ApplicationPluginEntry,
  AuditEntry,
  BuildSummary,
  ConfigMapDetail,
  CronJobSummary,
  DeckwatchSettings,
  DeploymentDetailResponse,
  DeploymentTemplate,
  DeprovisionResponse,
  DiagnosticHistoryItem,
  EventSummary,
  GitOpsStatus,
  HpaResponse,
  IngressClassSummary,
  IngressDetail,
  IngressTemplate,
  MarketplaceCatalog,
  McpActivityResponse,
  NodeSummary,
  OutboundStatus,
  PluginEventReport,
  PluginStatus,
  PluginSummary,
  PodSummary,
  ProvisionedResource,
  RevisionSummary,
  SecretDetail,
  ServiceAccountDetail,
  StorageClassSummary,
} from "@/types/api";
import type { ManifestDetail, RepositorySummary, TagSummary } from "@/api/registry";
import type { NodeUsage, PodUsage } from "@/api/resourceMetrics";
import type { MonitorSettings } from "@/types/monitoring";

/**
 * Deterministic data behind the e2e API mocks.
 *
 * Every timestamp is relative to FIXED_NOW, and the browser clock is frozen at
 * FIXED_NOW (see fixtures.ts), so relative ages ("3h", "2d ago") render the
 * same on every run. Nothing here may depend on the real clock or randomness.
 *
 * `createMockState()` returns a fresh deep copy per test, so a test that
 * mutates state (creating a namespace, saving settings) cannot leak into
 * another test.
 */

export const FIXED_NOW = new Date("2026-06-01T12:00:00.000Z");

/** ISO timestamp `d` days, `h` hours, `m` minutes, `s` seconds before FIXED_NOW. */
export function ago({ d = 0, h = 0, m = 0, s = 0 }: { d?: number; h?: number; m?: number; s?: number }): string {
  const ms = (((d * 24 + h) * 60 + m) * 60 + s) * 1000;
  return new Date(FIXED_NOW.getTime() - ms).toISOString();
}

const MARKETPLACE_URL = "https://market.example.test/catalog.json";

// ---------------------------------------------------------------- settings

function settings(): DeckwatchSettings {
  return {
    allowed_namespaces: ["default", "staging", "team-a"],
    default_resource_limits: {
      cpu_request: "100m",
      memory_request: "128Mi",
      cpu_limit: "500m",
      memory_limit: "512Mi",
    },
    auth: null,
    notifications: {
      enabled: true,
      webhook_url: "configured",
      event_types: ["build_failed", "pod_crash_loop", "deployment_deleted"],
      namespaces: ["default"],
    },
    git_repositories: [
      { name: "storefront", url: "https://github.com/acme/storefront.git", default_branch: "main" },
      { name: "platform-api", url: "https://github.com/acme/platform-api.git", default_branch: "develop" },
    ],
    oci_registries: [
      { name: "deckwatch", url: "https://registry.deckwatch.local/", registry_type: "deckwatch", builtin: true },
      { name: "ecr-prod", url: "123456789012.dkr.ecr.us-east-1.amazonaws.com", registry_type: "ecr" },
      { name: "ghcr", url: "ghcr.io/acme", registry_type: "ghcr" },
    ],
    git_token_secrets: [
      { name: "github-bot", secret_name: "github-bot-token", namespace: "deckwatch" },
      { name: "gitlab-ci", secret_name: "gitlab-ci-token", namespace: "deckwatch" },
    ],
    prometheus_enabled: true,
    ai_claude_enabled: true,
    ai_codex_enabled: false,
    agent_feedback_enabled: true,
    ai_provider: { type: "native", api_key_secret: "deckwatch-anthropic-api-key" },
    cost: { cost_per_cpu_hour: 0.04, cost_per_gb_hour: 0.005, currency: "USD" },
    tracing: null,
    credentials: { anthropic_api_key: "configured", gcp_sa_key: null },
    default_storage_class: "gp3",
    ingress_templates: [
      {
        name: "public-alb",
        ingress_class: "alb",
        annotations: {
          "alb.ingress.kubernetes.io/scheme": "internet-facing",
          "alb.ingress.kubernetes.io/target-type": "ip",
        },
        is_default: true,
      },
      { name: "internal-nginx", ingress_class: "nginx", annotations: {}, is_default: false },
    ],
    plugins: [
      {
        name: "otel",
        enabled: true,
        source: { type: "github", repo: "deckwatch/deckwatch-plugin-otel", ref: "v1.2.0", path: "", use_release: true },
        token_secret: null,
        allowed_hosts: ["otel-collector.observability.svc", "api.honeycomb.io"],
        config: { exporter: "otlp", api_key: "configured" },
        inherit_env_keys: ["OTEL_EXPORTER_OTLP_HEADERS"],
        inherit_env_file_keys: { aws_web_identity_token: "AWS_WEB_IDENTITY_TOKEN_FILE" },
        sha256: "3f7a9c2e5b8d1f4a6c0e2b9d7f5a3c1e8b6d4f2a0c9e7b5d3f1a8c6e4b2d0f9a",
      },
      {
        name: "aws-rds",
        enabled: false,
        source: { type: "url", url: "https://plugins.example.test/aws-rds-0.4.1.wasm" },
        token_secret: null,
        allowed_hosts: ["rds.us-east-1.amazonaws.com"],
        config: {},
        inherit_env_keys: [],
        inherit_env_file_keys: {},
      },
    ],
    mcp_tuning: {
      global: "Prefer read-only tools. Confirm before deleting anything.",
      deployments: "Always check metrics before recommending resource changes.",
    },
    marketplace_url: MARKETPLACE_URL,
  };
}

// --------------------------------------------------------------- workloads

const condition = (condition_type: string, status: boolean, reason: string | null = null, message: string | null = null) => ({
  condition_type,
  status,
  reason,
  message,
});

function runningPod(name: string, container: string, image: string, node: string, startedAgo: string, restarts = 0): PodSummary {
  return {
    name,
    phase: "Running",
    ready: true,
    restart_count: restarts,
    node,
    started_at: startedAgo,
    conditions: [
      condition("PodScheduled", true),
      condition("Initialized", true),
      condition("ContainersReady", true),
      condition("Ready", true),
    ],
    container_statuses: [
      { name: container, ready: true, restart_count: restarts, state: "running", state_reason: null, image },
    ],
  };
}

function deploymentBase(ns: string, name: string, image: string, overrides: Partial<DeploymentDetailResponse> = {}): DeploymentDetailResponse {
  return {
    name,
    namespace: ns,
    image,
    replicas: { desired: 1, ready: 1, available: 1, updated: 1 },
    status: "available",
    created_at: ago({ d: 12 }),
    labels: { app: name },
    annotations: {},
    conditions: [
      { condition_type: "Available", status: "True", reason: "MinimumReplicasAvailable", message: "Deployment has minimum availability.", last_transition: ago({ d: 2 }) },
      { condition_type: "Progressing", status: "True", reason: "NewReplicaSetAvailable", message: `ReplicaSet "${name}-6c9f7d8b5" has successfully progressed.`, last_transition: ago({ d: 2 }) },
    ],
    env: [],
    command: [],
    args: [],
    resource_limits: null,
    resource_requests: null,
    liveness_probe: null,
    readiness_probe: null,
    startup_probe: null,
    pod_labels: { app: name },
    pod_annotations: {},
    node_selector: {},
    node_affinity: null,
    pods: [],
    ingresses: [],
    ...overrides,
  };
}

function defaultDeployments(): DeploymentDetailResponse[] {
  return [
    deploymentBase("default", "web", "nginx:1.25", {
      replicas: { desired: 3, ready: 3, available: 3, updated: 3 },
      created_at: ago({ d: 3, h: 4 }),
      labels: { app: "web", "app.kubernetes.io/part-of": "storefront", "deckwatch.io/application": "storefront" },
      env: [
        { name: "PORT", value: "8080" },
        { name: "LOG_LEVEL", value: "info" },
      ],
      resource_requests: { cpu: "250m", memory: "256Mi" },
      resource_limits: { cpu: "500m", memory: "512Mi" },
      liveness_probe: { probe_type: "httpGet", path: "/healthz", port: 8080, command: null, initial_delay_seconds: 10, period_seconds: 15, timeout_seconds: 2, failure_threshold: 3, success_threshold: null },
      readiness_probe: { probe_type: "httpGet", path: "/ready", port: 8080, command: null, initial_delay_seconds: 5, period_seconds: 10, timeout_seconds: 2, failure_threshold: 3, success_threshold: 1 },
      pod_labels: { app: "web", tier: "frontend" },
      pod_annotations: { "prometheus.io/scrape": "true" },
      node_selector: { "kubernetes.io/os": "linux" },
      node_affinity: {
        required: [{ match_expressions: [{ key: "topology.kubernetes.io/zone", operator: "In", values: ["us-east-1a", "us-east-1b"] }] }],
        preferred: [{ weight: 50, match_expressions: [{ key: "node.kubernetes.io/instance-type", operator: "In", values: ["m6i.large"] }] }],
      },
      pods: [
        runningPod("web-6c9f7d8b5-4xk2p", "web", "nginx:1.25", "ip-10-0-1-21.ec2.internal", ago({ d: 2, h: 3 })),
        runningPod("web-6c9f7d8b5-9bqzt", "web", "nginx:1.25", "ip-10-0-2-37.ec2.internal", ago({ d: 2, h: 3 })),
        runningPod("web-6c9f7d8b5-mr7wd", "web", "nginx:1.25", "ip-10-0-1-21.ec2.internal", ago({ d: 2, h: 3 }), 1),
      ],
      ingresses: [
        { name: "web-ingress", namespace: "default", hosts: ["shop.example.com"], ingress_class: "alb", created_at: ago({ d: 3 }), labels: {}, addresses: ["k8s-default-web-0a1b2c3d4e.us-east-1.elb.amazonaws.com"] },
      ],
    }),
    deploymentBase("default", "api", "ghcr.io/acme/platform-api:v2.4.1", {
      replicas: { desired: 2, ready: 2, available: 2, updated: 2 },
      created_at: ago({ d: 20 }),
      labels: { app: "api", "deckwatch.io/application": "storefront" },
      resource_requests: { cpu: "500m", memory: "1Gi" },
      resource_limits: { cpu: "1", memory: "2Gi" },
      pods: [
        runningPod("api-7f6d5c4b3-2hjkl", "api", "ghcr.io/acme/platform-api:v2.4.1", "ip-10-0-2-37.ec2.internal", ago({ d: 1, h: 6 })),
        runningPod("api-7f6d5c4b3-8mnpq", "api", "ghcr.io/acme/platform-api:v2.4.1", "ip-10-0-3-12.ec2.internal", ago({ d: 1, h: 6 })),
      ],
    }),
    deploymentBase("default", "worker", "ghcr.io/acme/worker:v1.8.0", {
      replicas: { desired: 2, ready: 1, available: 1, updated: 2 },
      status: "degraded",
      created_at: ago({ h: 9, m: 30 }),
      labels: { app: "worker", "deckwatch.io/application": "billing" },
      resource_requests: { cpu: "200m", memory: "512Mi" },
      conditions: [
        { condition_type: "Available", status: "False", reason: "MinimumReplicasUnavailable", message: "Deployment does not have minimum availability.", last_transition: ago({ m: 42 }) },
        { condition_type: "Progressing", status: "True", reason: "ReplicaSetUpdated", message: 'ReplicaSet "worker-5b8c9d7f6" is progressing.', last_transition: ago({ m: 42 }) },
      ],
      pods: [
        runningPod("worker-5b8c9d7f6-qwert", "worker", "ghcr.io/acme/worker:v1.8.0", "ip-10-0-3-12.ec2.internal", ago({ h: 9 })),
        {
          name: "worker-5b8c9d7f6-zx9vb",
          phase: "Running",
          ready: false,
          restart_count: 7,
          node: "ip-10-0-1-21.ec2.internal",
          started_at: ago({ h: 9 }),
          oom_killed: true,
          conditions: [
            condition("PodScheduled", true),
            condition("Initialized", true),
            condition("ContainersReady", false, "ContainersNotReady", "containers with unready status: [worker]"),
            condition("Ready", false, "ContainersNotReady", "containers with unready status: [worker]"),
          ],
          container_statuses: [
            { name: "worker", ready: false, restart_count: 7, state: "waiting", state_reason: "CrashLoopBackOff", image: "ghcr.io/acme/worker:v1.8.0", oom_killed: true },
          ],
        },
      ],
    }),
    deploymentBase("default", "reports", "ghcr.io/acme/reports:2026.05.30", {
      replicas: { desired: 1, ready: 0, available: 0, updated: 1 },
      status: "progressing",
      created_at: ago({ m: 4 }),
      labels: { app: "reports" },
      pods: [
        {
          name: "reports-84d6f5c7b9-l2m3n",
          phase: "Pending",
          ready: false,
          restart_count: 0,
          node: null,
          started_at: null,
          conditions: [condition("PodScheduled", false, "Unschedulable", "0/3 nodes are available: 3 Insufficient memory.")],
          container_statuses: [],
        },
      ],
    }),
    deploymentBase("default", "importer", "ghcr.io/acme/importer:latest", {
      replicas: { desired: 1, ready: 0, available: 0, updated: 1 },
      status: "failed",
      created_at: ago({ d: 1, h: 2 }),
      labels: { app: "importer" },
      conditions: [
        { condition_type: "Available", status: "False", reason: "MinimumReplicasUnavailable", message: "Deployment does not have minimum availability.", last_transition: ago({ d: 1 }) },
        { condition_type: "Progressing", status: "False", reason: "ProgressDeadlineExceeded", message: 'ReplicaSet "importer-6f7g8h9j0" has timed out progressing.', last_transition: ago({ d: 1 }) },
      ],
      pods: [
        {
          name: "importer-6f7g8h9j0-abcde",
          phase: "Pending",
          ready: false,
          restart_count: 0,
          node: "ip-10-0-2-37.ec2.internal",
          started_at: null,
          conditions: [condition("PodScheduled", true), condition("Initialized", true), condition("ContainersReady", false), condition("Ready", false)],
          container_statuses: [
            { name: "importer", ready: false, restart_count: 0, state: "waiting", state_reason: "ImagePullBackOff", image: "ghcr.io/acme/importer:latest" },
          ],
        },
      ],
    }),
    deploymentBase("default", "legacy-admin", "acme/legacy-admin:3.1", {
      replicas: { desired: 0, ready: 0, available: 0, updated: 0 },
      status: "scaled_to_zero",
      created_at: ago({ d: 41 }),
      labels: { app: "legacy-admin" },
    }),
  ];
}

function cronjobs(ns: string): CronJobSummary[] {
  if (ns !== "default") return [];
  return [
    { name: "nightly-backup", namespace: ns, schedule: "0 2 * * *", suspend: false, active_count: 0, last_schedule_time: ago({ h: 10 }), created_at: ago({ d: 30 }), labels: { "deckwatch.io/application": "storefront" } },
    { name: "cache-warmer", namespace: ns, schedule: "*/15 * * * *", suspend: false, active_count: 1, last_schedule_time: ago({ m: 5 }), created_at: ago({ d: 6 }), labels: {} },
    { name: "quarterly-report", namespace: ns, schedule: "0 6 1 */3 *", suspend: true, active_count: 0, last_schedule_time: null, created_at: ago({ d: 90 }), labels: {} },
  ];
}

function secrets(ns: string): SecretDetail[] {
  if (ns !== "default") return [];
  return [
    { name: "api-credentials", namespace: ns, secret_type: "Opaque", keys: ["API_KEY", "API_SECRET"], created_at: ago({ d: 12 }), data: { API_KEY: "ak_live_51H8xK2eZvKYlo2C", API_SECRET: "s3cr3t-value-for-e2e" } },
    { name: "registry-pull", namespace: ns, secret_type: "kubernetes.io/dockerconfigjson", keys: [".dockerconfigjson"], created_at: ago({ d: 40 }), data: { ".dockerconfigjson": '{"auths":{"ghcr.io":{"auth":"ZTJlOnRva2Vu"}}}' } },
    { name: "web-tls", namespace: ns, secret_type: "kubernetes.io/tls", keys: ["tls.crt", "tls.key"], created_at: ago({ d: 3 }), data: { "tls.crt": "-----BEGIN CERTIFICATE-----", "tls.key": "-----BEGIN PRIVATE KEY-----" } },
  ];
}

function configmaps(ns: string): ConfigMapDetail[] {
  if (ns !== "default") return [];
  return [
    { name: "app-config", namespace: ns, keys: ["LOG_LEVEL", "FEATURE_FLAGS"], created_at: ago({ d: 12 }), data: { LOG_LEVEL: "info", FEATURE_FLAGS: "checkout_v2=true\nnew_search=false" } },
    { name: "nginx-conf", namespace: ns, keys: ["nginx.conf"], created_at: ago({ d: 3 }), data: { "nginx.conf": "server {\n  listen 8080;\n  location / { root /usr/share/nginx/html; }\n}" } },
    { name: "kube-root-ca.crt", namespace: ns, keys: ["ca.crt"], created_at: ago({ d: 120 }), data: { "ca.crt": "-----BEGIN CERTIFICATE-----" } },
    { name: "feature-toggles", namespace: ns, keys: [], created_at: ago({ h: 6 }), data: {} },
  ];
}

function serviceAccounts(ns: string): ServiceAccountDetail[] {
  if (ns !== "default") return [];
  return [
    { name: "default", namespace: ns, irsa_role_arn: null, created_at: ago({ d: 120 }), labels: {}, annotations: {} },
    { name: "api-s3-reader", namespace: ns, irsa_role_arn: "arn:aws:iam::123456789012:role/api-s3-reader", created_at: ago({ d: 20 }), labels: { app: "api" }, annotations: { "eks.amazonaws.com/role-arn": "arn:aws:iam::123456789012:role/api-s3-reader" } },
  ];
}

// ------------------------------------------------------------ applications

function summaryOf(d: DeploymentDetailResponse) {
  return {
    name: d.name,
    namespace: d.namespace,
    image: d.image,
    replicas: d.replicas,
    status: d.status,
    created_at: d.created_at,
    labels: d.labels,
    resource_requests: d.resource_requests,
  };
}

function applications(deps: DeploymentDetailResponse[], crons: CronJobSummary[]): ApplicationDetail[] {
  const dep = (name: string) => summaryOf(deps.find((d) => d.name === name)!);
  const cron = (name: string) => crons.find((c) => c.name === name)!;
  return [
    {
      name: "storefront",
      namespace: "default",
      description: "Customer-facing web shop and its public API.",
      created_at: ago({ d: 30 }),
      updated_at: ago({ h: 5 }),
      git: { repo_url: "https://github.com/acme/storefront.git", branch: "main", token_secret: "github-bot-token" },
      deployments: [dep("web"), dep("api")],
      cronjobs: [cron("nightly-backup")],
      health: "healthy",
    },
    {
      name: "billing",
      namespace: "default",
      description: "Invoice generation and payment reconciliation workers.",
      created_at: ago({ d: 14 }),
      updated_at: null,
      git: null,
      deployments: [dep("worker")],
      cronjobs: [],
      health: "degraded",
    },
    {
      name: "data-import",
      namespace: "default",
      description: "",
      created_at: ago({ d: 1, h: 2 }),
      updated_at: null,
      git: { repo_url: "https://github.com/acme/importer.git", branch: "release" },
      deployments: [dep("importer")],
      cronjobs: [],
      health: "unhealthy",
    },
    {
      name: "sandbox",
      namespace: "default",
      description: "Scratch space for experiments.",
      created_at: ago({ h: 3 }),
      updated_at: null,
      git: null,
      deployments: [],
      cronjobs: [],
      health: "empty",
    },
  ];
}

function applicationPlugins(): Record<string, ApplicationPluginEntry[]> {
  return {
    storefront: [
      { plugin_name: "otel", created_at: ago({ d: 9 }), is_loaded: true },
      { plugin_name: "aws-rds", created_at: ago({ d: 2 }), is_loaded: false },
    ],
  };
}

function provisionedResources(): Record<string, ProvisionedResource[]> {
  return {
    storefront: [
      {
        id: "res-0001",
        application_id: "app-storefront",
        plugin_name: "aws-rds",
        resource_id: "postgres",
        fields: { engine_version: "16.3", instance_class: "db.t4g.medium", multi_az: "true" },
        state: { endpoint: "storefront-db.c9akciq32.us-east-1.rds.amazonaws.com", port: "5432", secret: "storefront-db-credentials" },
        created_at: ago({ d: 2 }),
        updated_at: ago({ d: 2 }),
      },
    ],
  };
}

// ------------------------------------------------------------ plugin status

function pluginStatusRow(row: Partial<PluginStatus> & Pick<PluginStatus, "plugin">): PluginStatus {
  return {
    namespace: "",
    application: "",
    resource_id: "",
    health: "healthy",
    last_export: "metadata",
    last_events: [],
    first_failure_at: null,
    consecutive_failures: 0,
    last_success_at: ago({ m: 2 }),
    last_changed_at: ago({ d: 1, h: 4 }),
    updated_at: ago({ m: 2 }),
    ...row,
  };
}

/**
 * Every plugin target is healthy by default: the app bar has no Problems
 * button, and the application and plugin pages show green badges.
 */
function pluginStatus(): PluginStatus[] {
  return [
    pluginStatusRow({ plugin: "otel" }),
    pluginStatusRow({ plugin: "aws-rds" }),
    pluginStatusRow({ namespace: "default", application: "storefront", plugin: "otel", last_export: "apply" }),
    pluginStatusRow({
      namespace: "default",
      application: "storefront",
      plugin: "aws-rds",
      resource_id: "postgres",
      last_export: "fetch",
    }),
  ];
}

/**
 * Plugin targets in trouble, for the Problems views: storefront's database
 * failing, a plugin that no longer loads, and storefront's otel apply()
 * degraded. Replaces the matching healthy rows.
 */
export function pluginProblems(): PluginStatus[] {
  const trace = "4bf92f3577b34da6a3ce929d0e0e4736";
  return [
    pluginStatusRow({ plugin: "otel" }),
    pluginStatusRow({ plugin: "aws-rds" }),
    pluginStatusRow({
      namespace: "default",
      application: "storefront",
      plugin: "aws-rds",
      resource_id: "postgres",
      health: "failing",
      last_export: "fetch",
      consecutive_failures: 3,
      first_failure_at: ago({ m: 14 }),
      last_success_at: ago({ h: 2, m: 10 }),
      last_changed_at: ago({ m: 14 }),
      updated_at: ago({ m: 1 }),
      trace_id: trace,
      last_events: [
        {
          plugin: "aws-rds",
          export: "fetch",
          severity: "error",
          code: "aws-rds.describe_failed",
          message: "DescribeDBInstances returned AccessDenied for storefront-db.",
          resource_id: "postgres",
          retryable: true,
          hint: "Allow rds:DescribeDBInstances for the deckwatch IAM role.",
          trace_id: trace,
        },
      ],
    }),
    pluginStatusRow({
      plugin: "vault-secrets",
      health: "failing",
      last_export: "metadata",
      consecutive_failures: 1,
      first_failure_at: ago({ h: 1 }),
      last_success_at: null,
      last_changed_at: ago({ h: 1 }),
      updated_at: ago({ h: 1 }),
      last_events: [
        {
          plugin: "vault-secrets",
          export: "metadata",
          severity: "error",
          code: "plugin.trap",
          message: "wasm trap: unreachable instruction executed",
          retryable: false,
        },
      ],
    }),
    pluginStatusRow({
      namespace: "default",
      application: "storefront",
      plugin: "otel",
      health: "degraded",
      last_export: "apply",
      last_success_at: ago({ m: 1 }),
      last_changed_at: ago({ m: 6 }),
      updated_at: ago({ m: 1 }),
      last_events: [
        {
          plugin: "otel",
          export: "apply",
          severity: "warning",
          code: "otel.collector_unreachable",
          message: "otel-collector.observability.svc:4318 did not answer; spans are buffered in the pod.",
          retryable: true,
        },
        {
          plugin: "otel",
          export: "apply",
          severity: "info",
          code: "otel.sampling",
          message: "Head sampling is at 10%.",
          retryable: false,
        },
      ],
    }),
  ];
}

/**
 * What storefront's plugins report when a deployment is created or updated:
 * one of each severity (MockState.writePluginEvents attaches them).
 */
export function writePluginEvents(): PluginEventReport[] {
  return [
    {
      plugin: "otel",
      export: "apply",
      severity: "info",
      code: "otel.injected",
      message: "Added OTEL_EXPORTER_OTLP_ENDPOINT to 1 container.",
      retryable: false,
    },
    {
      plugin: "aws-rds",
      export: "apply",
      severity: "error",
      code: "aws-rds.secret_missing",
      message: "Secret storefront-db-credentials not found; DATABASE_URL was not set.",
      object: { api_version: "v1", kind: "Secret", name: "storefront-db-credentials" },
      retryable: false,
      hint: "Provision the PostgreSQL database again to recreate the Secret.",
    },
    {
      plugin: "otel",
      export: "apply",
      severity: "warning",
      code: "otel.collector_unreachable",
      message: "otel-collector.observability.svc:4318 did not answer; spans are buffered in the pod.",
      retryable: true,
    },
  ];
}

/** DELETE …/resources/aws-rds/postgres, when the plugin has things to say. */
export function deprovisionReport(): DeprovisionResponse {
  return {
    message: "Deleted the RDS instance storefront-db. Its final snapshot storefront-db-final is kept.",
    plugin_events: [
      {
        plugin: "aws-rds",
        export: "deprovision",
        severity: "warning",
        code: "aws-rds.snapshot_kept",
        message: "The final snapshot storefront-db-final is billed until you delete it.",
        resource_id: "postgres",
        retryable: false,
      },
    ],
    kubernetes_resource_deletions: [
      { api_version: "batch/v1", kind: "Job", name: "storefront-db-migrate", namespace: "default", outcome: "deleted" },
      { api_version: "batch/v1", kind: "Job", name: "storefront-db-backup", namespace: "default", outcome: "not_found" },
      {
        api_version: "v1",
        kind: "ServiceAccount",
        name: "storefront-db",
        namespace: "default",
        outcome: "refused",
        reason: "still used by deployment/web; deleting it would stop new pods from being created",
      },
    ],
  };
}

// ---------------------------------------------------------------- outbound

/**
 * GET /api/settings/outbound: every endpoint at its default, and the stored
 * Anthropic key bound to the default host (the git tokens are referenced
 * Secrets, not stored credentials).
 */
function outbound(): OutboundStatus {
  const endpoint = (
    id: OutboundStatus["endpoints"][number]["id"],
    setting: OutboundStatus["endpoints"][number]["setting"],
    label: string,
    url: string,
  ): OutboundStatus["endpoints"][number] => ({
    id,
    setting,
    label,
    default: url,
    value: "",
    effective: url,
    host: url.replace(/^https:\/\//, "").replace(/\/.*$/, ""),
    error: null,
  });
  return {
    allow_insecure_endpoints: false,
    extra_ca: null,
    endpoints: [
      endpoint("anthropic_api", "anthropic_api_base", "Anthropic API", "https://api.anthropic.com"),
      endpoint("vertex_token", "vertex_token_url", "Vertex AI token", "https://oauth2.googleapis.com/token"),
      endpoint("vertex_api", "vertex_api_base", "Vertex AI API", "https://{region}-aiplatform.googleapis.com"),
      endpoint("bedrock", "bedrock_endpoint", "Bedrock runtime", "https://bedrock-runtime.{region}.amazonaws.com"),
      endpoint("github_api", "github_api_base", "GitHub API", "https://api.github.com"),
      endpoint("github_web", "github_web_base", "GitHub web", "https://github.com"),
      endpoint("github_raw", "github_raw_base", "GitHub raw content", "https://raw.githubusercontent.com"),
    ],
    credentials: [
      {
        credential: "anthropic_api_key",
        label: "Anthropic API key",
        usable: true,
        bindings: [
          { endpoint: "anthropic_api", bound_host: "api.anthropic.com", configured_host: "api.anthropic.com", matches: true },
        ],
      },
    ],
  };
}

/**
 * The Anthropic endpoint moved to a gateway after the key was saved, with
 * http:// allowed and an extra CA bundle: the key is no longer sent anywhere.
 * Pair with settings.endpoints.anthropic_api_base = GATEWAY_URL.
 */
export const GATEWAY_URL = "https://llm-gateway.platform.internal";

export function movedOutbound(): OutboundStatus {
  const status = outbound();
  const anthropic = status.endpoints[0];
  Object.assign(anthropic, { value: GATEWAY_URL, effective: GATEWAY_URL, host: "llm-gateway.platform.internal" });
  status.allow_insecure_endpoints = true;
  status.extra_ca = { file: "/etc/deckwatch/ca/corp-root.pem", certificates: 2 };
  status.credentials[0].usable = false;
  status.credentials[0].bindings[0].configured_host = "llm-gateway.platform.internal";
  status.credentials[0].bindings[0].matches = false;
  return status;
}

// ----------------------------------------------------------------- plugins

function plugins(): PluginSummary[] {
  return [
    {
      name: "otel",
      version: "1.2.0",
      description: "Export deckwatch traces and deployment events to an OpenTelemetry collector.",
      provides: ["tracing", "events"],
      depends_on: [],
      config_schema: [
        { key: "exporter", label: "Exporter", description: "Protocol used to ship spans.", field_type: "select", default: "otlp", required: true, options: ["otlp", "zipkin", "jaeger"] },
        { key: "endpoint", label: "Collector endpoint", description: "OTLP/HTTP endpoint of the collector.", field_type: "string", default: "http://otel-collector.observability.svc:4318", required: true, options: [] },
        { key: "api_key", label: "API key", description: "Sent as the x-honeycomb-team header.", field_type: "secret", default: null, required: false, options: [] },
        { key: "sample_all", label: "Sample every request", description: "Disable head sampling.", field_type: "bool", default: "false", required: false, options: [] },
        { key: "headers", label: "Extra headers", description: "Read from the pod environment.", field_type: "string", default: null, required: false, options: [], env_source: "OTEL_EXPORTER_OTLP_HEADERS" },
      ],
      resources: [],
      mcp_tuning_fields: [
        { key: "traces", label: "Trace tool guidance", description: "Extra instructions for the list_traces tool.", placeholder: "Prefer the last 15 minutes unless asked otherwise.", default: null },
      ],
      wasm_size_bytes: 1_843_200,
      sha256: "3f7a9c2e5b8d1f4a6c0e2b9d7f5a3c1e8b6d4f2a0c9e7b5d3f1a8c6e4b2d0f9a",
      loaded_at: ago({ d: 1, h: 4 }),
    },
    {
      name: "aws-rds",
      version: "0.4.1",
      description: "Provision Amazon RDS databases for applications.",
      provides: ["resources"],
      depends_on: [],
      config_schema: [],
      resources: [
        {
          id: "postgres",
          label: "PostgreSQL database",
          icon: "mdi-database",
          description: "A managed PostgreSQL instance with credentials stored as a Secret.",
          singleton: true,
          fields: [
            { key: "engine_version", label: "Engine version", description: "PostgreSQL major.minor version.", field_type: "select", default: "16.3", required: true, options: ["16.3", "15.7", "14.12"] },
            { key: "instance_class", label: "Instance class", description: "RDS instance size.", field_type: "select", default: null, required: true, options: ["db.t4g.micro", "db.t4g.medium", "db.m6g.large"] },
            { key: "multi_az", label: "Multi-AZ", description: "Run a standby in a second zone.", field_type: "bool", default: "true", required: false, options: [] },
            { key: "master_password", label: "Master password", description: "Leave blank to generate one.", field_type: "secret", default: null, required: false, options: [] },
          ],
          output_keys: ["endpoint", "port", "secret"],
        },
        {
          id: "redis",
          label: "Redis cache",
          icon: "mdi-memory",
          description: "An ElastiCache Redis replication group.",
          singleton: false,
          fields: [
            { key: "node_type", label: "Node type", description: "Cache node size.", field_type: "select", default: "cache.t4g.small", required: true, options: ["cache.t4g.small", "cache.m6g.large"] },
          ],
          output_keys: ["endpoint"],
        },
      ],
      mcp_tuning_fields: [],
      wasm_size_bytes: 2_621_440,
      sha256: "9d0c8b7a6f5e4d3c2b1a0f9e8d7c6b5a4f3e2d1c0b9a8f7e6d5c4b3a2f1e0d9c",
      loaded_at: ago({ d: 1, h: 4 }),
    },
  ];
}

function marketplace(): MarketplaceCatalog {
  return {
    version: 1,
    updated_at: ago({ d: 3 }),
    plugins: [
      {
        name: "OpenTelemetry",
        slug: "deckwatch-plugin-otel",
        description: "Export traces and deployment events to any OpenTelemetry collector.",
        author: "deckwatch",
        homepage: "https://github.com/deckwatch/deckwatch-plugin-otel",
        trust_level: "verified",
        tags: ["observability", "tracing"],
        latest_version: "1.2.0",
        source: { type: "github", repo: "deckwatch/deckwatch-plugin-otel", ref: "v1.2.0", path: "", use_release: true },
        allowed_hosts_hint: ["otel-collector.observability.svc"],
        sha256: "3f7a9c2e5b8d1f4a6c0e2b9d7f5a3c1e8b6d4f2a0c9e7b5d3f1a8c6e4b2d0f9a",
      },
      {
        name: "Slack notifications",
        slug: "deckwatch-plugin-slack",
        description: "Post build and rollout events to Slack channels.",
        author: "deckwatch",
        homepage: "https://github.com/deckwatch/deckwatch-plugin-slack",
        trust_level: "verified",
        tags: ["notifications", "chatops"],
        latest_version: "0.9.3",
        source: { type: "github", repo: "deckwatch/deckwatch-plugin-slack", ref: "v0.9.3", path: "", use_release: true },
        allowed_hosts_hint: ["hooks.slack.com"],
      },
      {
        name: "Cost explorer",
        slug: "kubecost-bridge",
        description: "Pull per-workload cost allocation from a Kubecost installation.",
        author: "community-maintainers",
        homepage: "https://example.test/kubecost-bridge",
        trust_level: "community",
        tags: ["cost", "observability"],
        latest_version: "0.2.0",
        source: { type: "url", url: "https://example.test/kubecost-bridge-0.2.0.wasm" },
        allowed_hosts_hint: ["kubecost.kubecost.svc"],
      },
    ],
  };
}

// ------------------------------------------------------------- cluster-wide

function nodes(): NodeSummary[] {
  const ready = (name: string, zone: string, createdDays: number): NodeSummary => ({
    name,
    status: "Ready",
    roles: ["worker"],
    cpu_capacity: "4",
    memory_capacity: "16069840Ki",
    cpu_allocatable: "3920m",
    memory_allocatable: "15130768Ki",
    os_image: "Amazon Linux 2023.4.20240611",
    kernel_version: "6.1.92-99.174.amzn2023.x86_64",
    kubelet_version: "v1.32.3-eks-1234567",
    conditions: [
      { condition_type: "MemoryPressure", status: "False", reason: "KubeletHasSufficientMemory", message: "kubelet has sufficient memory available", last_transition: ago({ d: createdDays }) },
      { condition_type: "DiskPressure", status: "False", reason: "KubeletHasNoDiskPressure", message: "kubelet has no disk pressure", last_transition: ago({ d: createdDays }) },
      { condition_type: "Ready", status: "True", reason: "KubeletReady", message: `kubelet is posting ready status (${zone})`, last_transition: ago({ d: createdDays }) },
    ],
    created_at: ago({ d: createdDays }),
  });
  return [
    ready("ip-10-0-1-21.ec2.internal", "us-east-1a", 21),
    ready("ip-10-0-2-37.ec2.internal", "us-east-1b", 21),
    {
      ...ready("ip-10-0-3-12.ec2.internal", "us-east-1c", 2),
      status: "NotReady",
      conditions: [
        { condition_type: "Ready", status: "Unknown", reason: "NodeStatusUnknown", message: "Kubelet stopped posting node status.", last_transition: ago({ m: 18 }) },
      ],
    },
  ];
}

function nodeMetrics(): NodeUsage[] {
  return [
    { name: "ip-10-0-1-21.ec2.internal", timestamp: ago({ s: 15 }), window: "20s", cpu: "1240m", cpu_millicores: 1240, memory: "6291456Ki", memory_bytes: 6_442_450_944 },
    { name: "ip-10-0-2-37.ec2.internal", timestamp: ago({ s: 15 }), window: "20s", cpu: "2810m", cpu_millicores: 2810, memory: "11534336Ki", memory_bytes: 11_811_160_064 },
  ];
}

function events(): EventSummary[] {
  const ev = (o: Partial<EventSummary> & Pick<EventSummary, "name" | "event_type" | "reason" | "message" | "involved_object_kind" | "involved_object_name">, at: string): EventSummary => ({
    namespace: "default",
    involved_object_namespace: "default",
    count: 1,
    first_timestamp: at,
    last_timestamp: at,
    source_component: "kubelet",
    source_host: "ip-10-0-1-21.ec2.internal",
    ...o,
  });
  return [
    ev({ name: "worker-5b8c9d7f6-zx9vb.17d2a", event_type: "Warning", reason: "BackOff", message: "Back-off restarting failed container worker in pod worker-5b8c9d7f6-zx9vb_default", involved_object_kind: "Pod", involved_object_name: "worker-5b8c9d7f6-zx9vb", count: 31 }, ago({ m: 2 })),
    ev({ name: "importer-6f7g8h9j0-abcde.17d29", event_type: "Warning", reason: "Failed", message: 'Failed to pull image "ghcr.io/acme/importer:latest": not found', involved_object_kind: "Pod", involved_object_name: "importer-6f7g8h9j0-abcde", count: 12, source_host: "ip-10-0-2-37.ec2.internal" }, ago({ m: 6 })),
    ev({ name: "reports-84d6f5c7b9-l2m3n.17d2b", event_type: "Warning", reason: "FailedScheduling", message: "0/3 nodes are available: 3 Insufficient memory.", involved_object_kind: "Pod", involved_object_name: "reports-84d6f5c7b9-l2m3n", source_component: "default-scheduler", source_host: null }, ago({ m: 4 })),
    ev({ name: "web.17d1f", event_type: "Normal", reason: "ScalingReplicaSet", message: "Scaled up replica set web-6c9f7d8b5 to 3", involved_object_kind: "Deployment", involved_object_name: "web", source_component: "deployment-controller", source_host: null }, ago({ h: 2 })),
    ev({ name: "web-6c9f7d8b5-mr7wd.17d20", event_type: "Normal", reason: "Pulled", message: 'Container image "nginx:1.25" already present on machine', involved_object_kind: "Pod", involved_object_name: "web-6c9f7d8b5-mr7wd" }, ago({ h: 2 })),
    ev({ name: "ip-10-0-3-12.17d2c", namespace: "default", involved_object_namespace: null, event_type: "Warning", reason: "NodeNotReady", message: "Node ip-10-0-3-12.ec2.internal status is now: NodeNotReady", involved_object_kind: "Node", involved_object_name: "ip-10-0-3-12.ec2.internal", source_component: "node-controller", source_host: null }, ago({ m: 18 })),
  ];
}

function podMetrics(deps: DeploymentDetailResponse[]): PodUsage[] {
  const usage: Record<string, [number, number]> = {
    web: [42, 96],
    api: [180, 612],
    worker: [95, 401],
  };
  const out: PodUsage[] = [];
  for (const d of deps) {
    const u = usage[d.name];
    if (!u) continue;
    d.pods
      .filter((p) => p.phase === "Running")
      .forEach((p, i) => {
        const cpu = u[0] + i * 7;
        const memMiB = u[1] + i * 12;
        out.push({
          name: p.name,
          namespace: d.namespace,
          timestamp: ago({ s: 20 }),
          window: "20s",
          containers: [{ name: d.name, cpu: `${cpu}m`, cpu_millicores: cpu, memory: `${memMiB}Mi`, memory_bytes: memMiB * 1024 * 1024 }],
          total_cpu_millicores: cpu,
          total_memory_bytes: memMiB * 1024 * 1024,
          restart_count: p.restart_count,
        });
      });
  }
  return out;
}

// ---------------------------------------------------- deployment detail data

function hpa(): Record<string, HpaResponse> {
  return {
    web: {
      min_replicas: 2,
      max_replicas: 6,
      target_cpu_utilization: 70,
      current_cpu_utilization: 34,
      target_memory_utilization: null,
      current_memory_utilization: null,
      current_replicas: 3,
      desired_replicas: 3,
      conditions: [{ type: "AbleToScale", status: "True", reason: "ReadyForNewScale", message: "recommended size matches current size" }],
    },
  };
}

function gitops(): Record<string, GitOpsStatus> {
  return {
    web: {
      enabled: true,
      config: {
        repo_url: "https://github.com/acme/storefront.git",
        branch: "main",
        token_secret: "github-bot-token",
        git_auth_user: "x-access-token",
        dockerfile_path: "web/Dockerfile",
        docker_context: "web",
        ecr_repository: "",
        oci_repository: "registry.deckwatch.local/web",
        include_paths: ["web/**"],
        exclude_paths: ["**/*.md"],
        poll_interval_seconds: 60,
        webhook_enabled: true,
        webhook_secret_configured: true,
      },
      last_commit_sha: "9f8e7d6c5b4a39281706f5e4d3c2b1a098765432",
      last_build_status: "success",
      last_build_job: "web-build-9f8e7d6",
      last_build_time: ago({ h: 5 }),
      last_build_error: null,
    },
  };
}

function builds(): BuildSummary[] {
  return [
    { job_name: "web-build-9f8e7d6", commit_sha: "9f8e7d6c5b4a39281706f5e4d3c2b1a098765432", status: "success", started_at: ago({ h: 5, m: 4 }), completed_at: ago({ h: 5 }), image_tag: "9f8e7d6" },
    { job_name: "web-build-1a2b3c4", commit_sha: "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d", status: "failed", started_at: ago({ d: 1, h: 2 }), completed_at: ago({ d: 1, h: 1, m: 57 }), image_tag: "1a2b3c4" },
    { job_name: "web-build-0f1e2d3", commit_sha: "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c", status: "success", started_at: ago({ d: 2, h: 3 }), completed_at: ago({ d: 2, h: 2, m: 55 }), image_tag: "0f1e2d3" },
  ];
}

function monitor(): Record<string, MonitorSettings> {
  return {
    web: { enabled: true, name: "web", namespace: "default", port: "http", path: "/metrics", interval: "30s", matching_pods: 3, unavailable_reason: null },
  };
}

function history(): Record<string, RevisionSummary[]> {
  return {
    web: [
      { revision: 7, replica_set_name: "web-6c9f7d8b5", image: "nginx:1.25", replicas: 3, ready_replicas: 3, created_at: ago({ d: 2, h: 3 }), change_cause: "Bump nginx to 1.25", is_current: true },
      { revision: 6, replica_set_name: "web-5d8e6c7a4", image: "nginx:1.24", replicas: 0, ready_replicas: 0, created_at: ago({ d: 9 }), change_cause: "Enable gzip", is_current: false },
      { revision: 5, replica_set_name: "web-4c7d5b6f3", image: "nginx:1.24", replicas: 0, ready_replicas: 0, created_at: ago({ d: 16 }), change_cause: null, is_current: false },
    ],
  };
}

const revisionYaml = (image: string, extra = "") => `apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: default
spec:
  replicas: 3
  selector:
    matchLabels:
      app: web
  template:
    metadata:
      labels:
        app: web
    spec:
      containers:
        - name: web
          image: ${image}
          ports:
            - containerPort: 8080
${extra}`;

export function deploymentYaml(name: string, image: string): string {
  return revisionYaml(image).replace(/web/g, name);
}

function ingressDetails(): IngressDetail[] {
  return [
    {
      name: "web-ingress",
      namespace: "default",
      hosts: ["shop.example.com"],
      ingress_class: "alb",
      created_at: ago({ d: 3 }),
      labels: {},
      addresses: ["k8s-default-web-0a1b2c3d4e.us-east-1.elb.amazonaws.com"],
      rules: [{ host: "shop.example.com", paths: [{ path: "/", path_type: "Prefix", service_name: "web", service_port: 8080 }] }],
      tls: [{ hosts: ["shop.example.com"], secret_name: "web-tls" }],
      annotations: { "alb.ingress.kubernetes.io/scheme": "internet-facing" },
    },
  ];
}

function logLines(pod: string): string[] {
  const lines: string[] = [];
  const levels = ["INFO", "INFO", "DEBUG", "INFO", "WARN", "INFO", "ERROR", "INFO"];
  for (let i = 0; i < 24; i++) {
    const t = new Date(FIXED_NOW.getTime() - (24 - i) * 7_000).toISOString();
    const level = levels[i % levels.length];
    const msg =
      level === "ERROR"
        ? `upstream timed out (110: Connection timed out) while reading response header from upstream, request: "GET /api/cart HTTP/1.1"`
        : level === "WARN"
          ? `slow request: GET /products?page=${i} took ${800 + i * 13}ms`
          : level === "DEBUG"
            ? `cache hit ratio=0.${90 + (i % 9)} keys=${1200 + i * 3}`
            : `GET /products/${100 + i} 200 ${12 + i}ms pod=${pod}`;
    lines.push(`${t} ${level} ${msg}`);
  }
  return lines;
}

// ---------------------------------------------------------------- registry

function registryRepos(): RepositorySummary[] {
  return [
    { name: "web", tag_count: 3, total_size: 187_695_104 },
    { name: "acme/platform-api", tag_count: 2, total_size: 412_090_368 },
    { name: "worker", tag_count: 1, total_size: 96_468_992 },
  ];
}

function registryTags(): Record<string, TagSummary[]> {
  const digest = (n: number) => `sha256:${"0123456789abcdef".repeat(4).slice(n, 64)}${"fedcba9876543210".slice(0, n)}`;
  return {
    web: [
      { tag: "9f8e7d6", digest: digest(1), media_type: "application/vnd.oci.image.manifest.v1+json", size: 62_914_560, created: ago({ h: 5 }) },
      { tag: "0f1e2d3", digest: digest(2), media_type: "application/vnd.oci.image.manifest.v1+json", size: 62_390_272, created: ago({ d: 2, h: 3 }) },
      { tag: "latest", digest: digest(1), media_type: "application/vnd.oci.image.manifest.v1+json", size: 62_914_560, created: ago({ h: 5 }) },
    ],
    "acme/platform-api": [
      { tag: "v2.4.1", digest: digest(3), media_type: "application/vnd.oci.image.index.v1+json", size: 206_045_184, created: ago({ d: 1, h: 6 }) },
      { tag: "v2.4.0", digest: digest(4), media_type: "application/vnd.oci.image.index.v1+json", size: 206_045_184, created: ago({ d: 8 }) },
    ],
    worker: [{ tag: "v1.8.0", digest: digest(5), media_type: "application/vnd.docker.distribution.manifest.v2+json", size: 96_468_992, created: ago({ h: 9, m: 31 }) }],
  };
}

export function manifestFor(name: string, tag: TagSummary): ManifestDetail {
  return {
    name,
    tag: tag.tag,
    digest: tag.digest,
    media_type: tag.media_type,
    config: { digest: "sha256:c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00", media_type: "application/vnd.oci.image.config.v1+json", size: 7_023 },
    layers: [
      { digest: "sha256:1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa", media_type: "application/vnd.oci.image.layer.v1.tar+gzip", size: 29_126_144 },
      { digest: "sha256:2222bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222bbbb", media_type: "application/vnd.oci.image.layer.v1.tar+gzip", size: 33_781_393 },
    ],
    total_size: tag.size,
    manifest: { schemaVersion: 2, mediaType: tag.media_type },
  };
}

// ------------------------------------------------------- settings resources

function storageClasses(): StorageClassSummary[] {
  return [
    { name: "gp3", provisioner: "ebs.csi.aws.com", reclaim_policy: "Delete", volume_binding_mode: "WaitForFirstConsumer", allow_volume_expansion: true, is_default: true, mount_options: null, parameters: { type: "gp3", encrypted: "true" } },
    { name: "io2-retain", provisioner: "ebs.csi.aws.com", reclaim_policy: "Retain", volume_binding_mode: "WaitForFirstConsumer", allow_volume_expansion: true, is_default: false, mount_options: ["noatime"], parameters: { type: "io2", iops: "4000" } },
    { name: "efs-shared", provisioner: "efs.csi.aws.com", reclaim_policy: null, volume_binding_mode: "Immediate", allow_volume_expansion: false, is_default: false, mount_options: null, parameters: null },
  ];
}

function ingressClasses(): IngressClassSummary[] {
  return [
    { name: "alb", controller: "ingress.k8s.aws/alb", is_default: true, parameters: { api_group: "elbv2.k8s.aws", kind: "IngressClassParams", name: "alb-public", namespace: null, scope: "Cluster" } },
    { name: "nginx", controller: "k8s.io/ingress-nginx", is_default: false, parameters: null },
  ];
}

function templates(): DeploymentTemplate[] {
  return [
    { builtin: true, id: "web-app", name: "Web Application", description: "A stateless HTTP service with a readiness probe on port 8080.", icon: "mdi-web", category: "web_app", payload: { image: "nginx:1.25", replicas: 2, ports: [{ port: 8080, name: "http" }], readiness_probe: { probe_type: "httpGet", path: "/", port: 8080 } } },
    { builtin: true, id: "worker", name: "Background Worker", description: "A queue consumer with no exposed ports.", icon: "mdi-cog-sync", category: "worker", payload: { image: "busybox:1.36", replicas: 1, command: ["sh", "-c", "sleep infinity"] } },
    { builtin: true, id: "static-site", name: "Static Site", description: "Serve static files with nginx.", icon: "mdi-file-document-multiple-outline", category: "static_site", payload: { image: "nginx:alpine", replicas: 1, ports: [{ port: 80 }] } },
    { builtin: false, id: "acme-node", name: "ACME Node.js service", description: "Company baseline for Node.js APIs: probes, limits and env.", icon: "mdi-nodejs", category: "web_app", payload: { image: "ghcr.io/acme/node-base:22", replicas: 2, ports: [{ port: 3000, name: "http" }], env: [{ name: "NODE_ENV", value: "production" }], resource_requests: { cpu: "100m", memory: "256Mi" }, resource_limits: { cpu: "500m", memory: "512Mi" } } },
  ];
}

function ingressTemplates(s: DeckwatchSettings): IngressTemplate[] {
  return s.ingress_templates ?? [];
}

// ------------------------------------------------------------- audit & misc

function audit(): AuditEntry[] {
  const e = (i: number, minutesAgo: number, action: string, resource_type: string, resource_name: string, namespace: string, detail: string, user = "alice@example.com"): AuditEntry => ({
    id: `audit-${String(i).padStart(4, "0")}`,
    timestamp: ago({ m: minutesAgo }),
    action,
    resource_type,
    resource_name,
    namespace,
    detail,
    user_identity: user,
    source: "api",
    tool: "",
    outcome: "",
    client_name: "",
  });
  return [
    e(1, 3, "scale", "deployment", "web", "default", "replicas 2 -> 3"),
    e(2, 18, "update", "deployment", "worker", "default", "image ghcr.io/acme/worker:v1.7.2 -> v1.8.0", "ci-bot"),
    e(3, 47, "create", "cronjob", "cache-warmer", "default", "schedule */15 * * * *"),
    e(4, 95, "restart", "deployment", "api", "default", "rolling restart"),
    e(5, 240, "delete", "secret", "old-registry-pull", "default", "", "bob@example.com"),
    e(6, 600, "create", "application", "storefront", "default", "2 deployments, 1 cronjob"),
    e(7, 1500, "update", "settings", "deckwatch", "", "allowed_namespaces updated", "bob@example.com"),
    e(8, 2900, "create", "namespace", "staging", "staging", ""),
    ...mcpAudit(),
  ];
}

/**
 * MCP rows (SEC-22): what MCP clients changed or were refused, with the JSON
 * detail the backend writes. Newest first, like the API.
 */
function mcpAudit(): AuditEntry[] {
  const m = (
    i: number,
    minutesAgo: number,
    row: Pick<AuditEntry, "action" | "tool" | "outcome" | "resource_type" | "resource_name" | "namespace" | "client_name">,
    detail: Record<string, unknown>,
  ): AuditEntry => ({
    id: `audit-mcp-${String(i).padStart(4, "0")}`,
    timestamp: ago({ m: minutesAgo }),
    user_identity: "",
    source: "mcp",
    ...row,
    detail: JSON.stringify({ tool: row.tool, outcome: row.outcome, argument_keys: [], arguments: {}, duration_ms: 40, ...detail }),
  });
  const claude = { name: "claude-code", version: "2.1.0", self_reported: true };
  return [
    m(1, 7, { action: "write", tool: "apply_manifest", outcome: "ok", resource_type: "deployment", resource_name: "web", namespace: "default", client_name: "claude-code" }, {
      argument_keys: ["manifest"],
      client: claude,
      trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
      duration_ms: 812,
    }),
    m(2, 26, { action: "delete", tool: "delete_pod", outcome: "denied", resource_type: "pod", resource_name: "etcd-0", namespace: "kube-system", client_name: "claude-code" }, {
      denied_rule: "namespace_allowlist",
      argument_keys: ["name", "namespace"],
      arguments: { name: "etcd-0", namespace: "kube-system" },
      client: claude,
    }),
    m(3, 64, { action: "exec", tool: "exec_pod", outcome: "ok", resource_type: "pod", resource_name: "web-6c9f7d8b5-4xk2p", namespace: "default", client_name: "cursor" }, {
      argument_keys: ["command", "name", "namespace"],
      arguments: { name: "web-6c9f7d8b5-4xk2p", namespace: "default" },
      client: { name: "cursor", version: "1.4.2", self_reported: true },
    }),
    m(4, 190, { action: "write", tool: "scale_deployment", outcome: "error", resource_type: "deployment", resource_name: "worker", namespace: "default", client_name: "" }, {
      argument_keys: ["name", "namespace", "replicas"],
      arguments: { name: "worker", namespace: "default", replicas: 40 },
    }),
  ];
}

/**
 * GET /api/prometheus/mcp_activity: tool calls following a daily curve
 * (busier in UTC working hours), a few errors and denials, and the per-tool
 * totals. Deterministic: no randomness, timestamps end at FIXED_NOW.
 */
export function mcpActivity(window: string): McpActivityResponse {
  const { seconds, step } = { "1h": { seconds: 3600, step: 60 }, "7d": { seconds: 604800, step: 1800 } }[window] ?? {
    seconds: 86400,
    step: 300,
  };
  const n = seconds / step;
  const at = (i: number) => FIXED_NOW.getTime() - (n - 1 - i) * step * 1000;
  const load = (t: number) => {
    const d = new Date(t);
    const hour = d.getUTCHours() + d.getUTCMinutes() / 60;
    return 2 + 8 * Math.max(0, Math.sin(((hour - 6) / 12) * Math.PI));
  };
  const ok = Array.from({ length: n }, (_, i) => ({ t: at(i), v: Number(load(at(i)).toFixed(2)) }));
  return {
    window_seconds: seconds,
    step_seconds: step,
    calls_per_minute: [
      { label: "ok", points: ok },
      { label: "error", points: ok.map((p, i) => ({ t: p.t, v: i % 37 === 5 ? 0.4 : 0 })) },
      { label: "denied", points: ok.map((p, i) => ({ t: p.t, v: i % 53 === 11 ? 0.2 : 0 })) },
    ],
    apply_manifest_per_minute: [{ label: "ok", points: ok.map((p) => ({ t: p.t, v: Number((p.v / 12).toFixed(3)) })) }],
    p95_seconds: ok.map((p) => ({ t: p.t, v: Number((0.18 + p.v / 40).toFixed(3)) })),
    tools: [
      { tool: "list_pods", calls: 1840, errors: 3, denied: 0, p95_seconds: 0.11 },
      { tool: "get_deployment", calls: 962, errors: 0, denied: 0, p95_seconds: 0.08 },
      { tool: "get_pod_logs", calls: 411, errors: 6, denied: 0, p95_seconds: 0.93 },
      { tool: "apply_manifest", calls: 57.4, errors: 1, denied: 2, p95_seconds: 0.81 },
      { tool: "exec_pod", calls: 12, errors: 0, denied: 1, p95_seconds: 2.4 },
      { tool: "delete_pod", calls: 4, errors: 0, denied: 4, p95_seconds: null },
    ],
  };
}

function agentFeedback(): AgentFeedbackItem[] {
  return [
    { id: "fb-001", created_at: ago({ h: 2 }), category: "missing_tool", summary: "No tool to read HPA events", detail: "While diagnosing a scaling issue I needed the HPA event history but only get_hpa exists.", suggested_tool_name: "get_hpa_events", suggested_prompt: "Return the last 20 events for the HPA targeting the deployment.", status: "pending", reviewed_at: null },
    { id: "fb-002", created_at: ago({ d: 1 }), category: "mcp_tuning", summary: "list_pods output is too verbose", detail: "Container statuses for every pod blow up the context window on large namespaces.", suggested_tool_name: null, suggested_prompt: null, status: "reviewed", reviewed_at: ago({ h: 20 }) },
    { id: "fb-003", created_at: ago({ d: 4 }), category: "documentation", summary: "GitOps webhook setup is unclear", detail: "The docs do not say which events the GitHub webhook needs.", suggested_tool_name: null, suggested_prompt: null, status: "actioned", reviewed_at: ago({ d: 3 }) },
  ];
}

function diagnostics(): DiagnosticHistoryItem[] {
  return [];
}

const docsPages = [
  { slug: "getting-started", title: "Getting started" },
  { slug: "gitops", title: "GitOps" },
  { slug: "applications", title: "Applications" },
];

const docsMarkdown: Record<string, string> = {
  "getting-started": `# Getting started

Deckwatch is a web console for **Kubernetes** deployments. This page walks you through the first steps.

## Install

Install the Helm chart into its own namespace:

\`\`\`
helm install deckwatch oci://ghcr.io/acme/charts/deckwatch \\
  --namespace deckwatch --create-namespace
\`\`\`

## First deployment

- Pick a namespace with the switcher in the app bar.
- Open *Resources* and choose **Create Deployment**.
- Fill in a name and an image such as \`nginx:1.25\`.

See [the GitOps guide](/docs/gitops) to build images from a repository.
`,
  gitops: `# GitOps

Point a deployment at a Git repository and deckwatch builds and rolls out every new commit.
`,
  applications: `# Applications

Group deployments and cronjobs that ship together.
`,
};

// ------------------------------------------------------------------- state

export interface MockState {
  namespaces: string[];
  features: { prometheus: boolean; registry: boolean };
  settings: DeckwatchSettings;
  deployments: DeploymentDetailResponse[];
  cronjobs: CronJobSummary[];
  secrets: SecretDetail[];
  configmaps: ConfigMapDetail[];
  serviceAccounts: ServiceAccountDetail[];
  applications: ApplicationDetail[];
  applicationPlugins: Record<string, ApplicationPluginEntry[]>;
  provisioned: Record<string, ProvisionedResource[]>;
  plugins: PluginSummary[];
  pluginStatus: PluginStatus[];
  /** Attached as plugin_events to deployment create and update responses. */
  writePluginEvents: PluginEventReport[];
  outbound: OutboundStatus;
  marketplace: MarketplaceCatalog;
  marketplaceUrl: string;
  nodes: NodeSummary[];
  nodeMetrics: NodeUsage[];
  events: EventSummary[];
  podMetrics: PodUsage[];
  hpa: Record<string, HpaResponse>;
  gitops: Record<string, GitOpsStatus>;
  builds: BuildSummary[];
  monitor: Record<string, MonitorSettings>;
  history: Record<string, RevisionSummary[]>;
  revisionYaml: Record<number, string>;
  ingresses: IngressDetail[];
  ingressTemplates: IngressTemplate[];
  ingressClasses: IngressClassSummary[];
  storageClasses: StorageClassSummary[];
  templates: DeploymentTemplate[];
  registryRepos: RepositorySummary[];
  registryTags: Record<string, TagSummary[]>;
  audit: AuditEntry[];
  agentFeedback: AgentFeedbackItem[];
  diagnostics: DiagnosticHistoryItem[];
  docsPages: { slug: string; title: string }[];
  docsMarkdown: Record<string, string>;
  logLines: (pod: string) => string[];
  branches: Record<string, string[]>;
}

export function createMockState(): MockState {
  const s = settings();
  const deps = defaultDeployments();
  const crons = cronjobs("default");
  return {
    namespaces: ["default", "staging", "team-a"],
    features: { prometheus: true, registry: true },
    settings: s,
    deployments: deps,
    cronjobs: crons,
    secrets: secrets("default"),
    configmaps: configmaps("default"),
    serviceAccounts: serviceAccounts("default"),
    applications: applications(deps, crons),
    applicationPlugins: applicationPlugins(),
    provisioned: provisionedResources(),
    plugins: plugins(),
    pluginStatus: pluginStatus(),
    writePluginEvents: [],
    outbound: outbound(),
    marketplace: marketplace(),
    marketplaceUrl: MARKETPLACE_URL,
    nodes: nodes(),
    nodeMetrics: nodeMetrics(),
    events: events(),
    podMetrics: podMetrics(deps),
    hpa: hpa(),
    gitops: gitops(),
    builds: builds(),
    monitor: monitor(),
    history: history(),
    revisionYaml: {
      7: revisionYaml("nginx:1.25", "          resources:\n            requests:\n              cpu: 250m\n"),
      6: revisionYaml("nginx:1.24"),
      5: revisionYaml("nginx:1.24"),
    },
    ingresses: ingressDetails(),
    ingressTemplates: ingressTemplates(s),
    ingressClasses: ingressClasses(),
    storageClasses: storageClasses(),
    templates: templates(),
    registryRepos: registryRepos(),
    registryTags: registryTags(),
    audit: audit(),
    agentFeedback: agentFeedback(),
    diagnostics: diagnostics(),
    docsPages: docsPages.map((p) => ({ ...p })),
    docsMarkdown: { ...docsMarkdown },
    logLines,
    branches: {
      "https://github.com/acme/storefront.git": ["main", "develop", "release/2026.06", "feature/checkout-v2"],
      "https://github.com/acme/platform-api.git": ["develop", "main"],
    },
  };
}
