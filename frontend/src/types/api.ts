export interface DeploymentSummary {
  name: string;
  namespace: string;
  image: string;
  replicas: ReplicaCounts;
  status: DeploymentPhase;
  created_at: string | null;
  labels: Record<string, string>;
}

export interface DeploymentDetail extends DeploymentSummary {
  conditions: DeploymentCondition[];
  env: EnvVar[];
  command: string[];
  args: string[];
  resource_limits: ResourceSpec | null;
  resource_requests: ResourceSpec | null;
  liveness_probe: ProbeConfig | null;
  readiness_probe: ProbeConfig | null;
  startup_probe: ProbeConfig | null;
}

export interface ProbeConfig {
  probe_type: string;
  path: string | null;
  port: number | null;
  command: string[] | null;
  initial_delay_seconds: number | null;
  period_seconds: number | null;
  timeout_seconds: number | null;
  failure_threshold: number | null;
  success_threshold: number | null;
}

export interface DeploymentDetailResponse extends DeploymentDetail {
  pods: PodSummary[];
  ingresses: IngressSummary[];
}

export interface ReplicaCounts {
  desired: number;
  ready: number;
  available: number;
  updated: number;
}

export type DeploymentPhase =
  | "available"
  | "progressing"
  | "degraded"
  | "failed"
  | "scaled_to_zero";

export interface DeploymentCondition {
  condition_type: string;
  status: string;
  reason: string | null;
  message: string | null;
  last_transition: string | null;
}

export interface EnvVar {
  name: string;
  value: string | null;
}

export interface ResourceSpec {
  cpu: string | null;
  memory: string | null;
}

export interface PodCondition {
  condition_type: string;
  status: boolean;
  reason: string | null;
  message: string | null;
}

export interface PodSummary {
  name: string;
  phase: string;
  ready: boolean;
  restart_count: number;
  node: string | null;
  started_at: string | null;
  conditions: PodCondition[];
  container_statuses: ContainerStatusSummary[];
}

export interface ContainerStatusSummary {
  name: string;
  ready: boolean;
  restart_count: number;
  state: string;
  state_reason: string | null;
  image: string;
}

export interface ProbeInput {
  probe_type: string;
  path?: string;
  port?: number;
  command?: string[];
  initial_delay_seconds?: number;
  period_seconds?: number;
  timeout_seconds?: number;
  failure_threshold?: number;
  success_threshold?: number;
}

export interface ContainerPortInput {
  port: number;
  name?: string;
  protocol?: string;
}

export interface CreateDeploymentRequest {
  name: string;
  image: string;
  replicas?: number;
  // `port` is kept for backward compatibility with older clients and templates;
  // new code should populate `ports` instead.
  port?: number;
  ports?: ContainerPortInput[];
  env?: { name: string; value: string }[];
  labels?: Record<string, string>;
  command?: string[];
  args?: string[];
  resource_limits?: ResourceSpec;
  resource_requests?: ResourceSpec;
  liveness_probe?: ProbeInput;
  readiness_probe?: ProbeInput;
  startup_probe?: ProbeInput;
}

export interface UpdateDeploymentRequest {
  image?: string;
  replicas?: number;
  port?: number;
  ports?: ContainerPortInput[];
  env?: { name: string; value: string }[];
  command?: string[];
  args?: string[];
  resource_limits?: ResourceSpec;
  resource_requests?: ResourceSpec;
  liveness_probe?: ProbeInput;
  readiness_probe?: ProbeInput;
  startup_probe?: ProbeInput;
}

export interface DeploymentListResponse {
  deployments: DeploymentSummary[];
}

export interface NamespaceListResponse {
  namespaces: string[];
}

export interface CreateNamespaceRequest {
  name: string;
  labels?: Record<string, string>;
}

export interface CreateNamespaceResponse {
  name: string;
  created_at: string | null;
  labels: Record<string, string>;
}

// --- Ingress types ---

export interface IngressSummary {
  name: string;
  namespace: string;
  hosts: string[];
  ingress_class: string | null;
  created_at: string | null;
  labels: Record<string, string>;
  addresses: string[];
}

export interface IngressDetail extends IngressSummary {
  rules: IngressRuleSummary[];
  tls: IngressTlsSummary[];
  annotations: Record<string, string>;
}

export interface IngressRuleSummary {
  host: string | null;
  paths: IngressPathSummary[];
}

export interface IngressPathSummary {
  path: string;
  path_type: string;
  service_name: string;
  service_port: number;
}

export interface IngressTlsSummary {
  hosts: string[];
  secret_name: string | null;
}

export interface CreateIngressRequest {
  name: string;
  host?: string;
  paths: {
    path: string;
    path_type?: string;
    service_name: string;
    service_port: number;
  }[];
  ingress_class?: string;
  annotations?: Record<string, string>;
  tls?: { hosts: string[]; secret_name?: string }[];
}

export interface IngressListResponse {
  ingresses: IngressSummary[];
}

// --- GitOps types ---

export interface GitOpsConfig {
  repo_url: string;
  branch: string;
  token_secret: string;
  dockerfile_path: string;
  docker_context: string;
  ecr_repository: string;
  oci_repository?: string;
  include_paths: string[];
  exclude_paths: string[];
  poll_interval_seconds: number;
  webhook_enabled: boolean;
  webhook_secret_configured?: boolean;
}

export interface GitOpsStatus {
  enabled: boolean;
  config: GitOpsConfig | null;
  last_commit_sha: string | null;
  last_build_status: "success" | "failed" | "building" | "pending" | null;
  last_build_job: string | null;
  last_build_time: string | null;
  last_build_error: string | null;
}

export interface GitOpsConfigRequest {
  repo_url: string;
  branch?: string;
  token_secret: string;
  dockerfile_path?: string;
  docker_context?: string;
  ecr_repository?: string;
  oci_repository?: string;
  include_paths?: string[];
  exclude_paths?: string[];
  poll_interval_seconds?: number;
  webhook_enabled?: boolean;
  webhook_secret?: string;
}

export interface BuildSummary {
  job_name: string;
  commit_sha: string;
  status: string;
  started_at: string | null;
  completed_at: string | null;
  image_tag: string;
}

export interface BuildListResponse {
  builds: BuildSummary[];
}

// --- CronJob types ---

export interface CronJobSummary {
  name: string;
  namespace: string;
  schedule: string;
  suspend: boolean;
  active_count: number;
  last_schedule_time: string | null;
  created_at: string | null;
  labels: Record<string, string>;
}

export interface CronJobListResponse {
  cronjobs: CronJobSummary[];
}

export interface CronJobDetailResponse extends CronJobSummary {}

// --- Node types ---

export interface NodeConditionSummary {
  condition_type: string;
  status: string;
  reason: string | null;
  message: string | null;
  last_transition: string | null;
}

export interface NodeSummary {
  name: string;
  status: string;
  roles: string[];
  cpu_capacity: string | null;
  memory_capacity: string | null;
  cpu_allocatable: string | null;
  memory_allocatable: string | null;
  os_image: string | null;
  kernel_version: string | null;
  kubelet_version: string | null;
  conditions: NodeConditionSummary[];
  created_at: string | null;
}

export interface NodeListResponse {
  nodes: NodeSummary[];
}

// --- Probe update ---

export interface UpdateProbesRequest {
  liveness_probe?: ProbeInput | null;
  readiness_probe?: ProbeInput | null;
  startup_probe?: ProbeInput | null;
}

// --- Custom sidecar containers ---

export interface AddContainerRequest {
  name: string;
  image: string;
  port?: number;
  env?: { name: string; value: string }[];
  command?: string[];
  args?: string[];
  resource_limits?: ResourceSpec;
  resource_requests?: ResourceSpec;
}

// --- Addons ---

export interface AddonEnvVar {
  name: string;
  value: string;
}

export interface AddonResourceSpec {
  cpu: string | null;
  memory: string | null;
}

export interface AddonDefinition {
  id: string;
  name: string;
  description: string;
  image: string;
  default_port: number | null;
  default_env: AddonEnvVar[];
  default_resources: AddonResourceSpec | null;
}

export interface AddonListResponse {
  addons: AddonDefinition[];
}

export interface AttachAddonRequest {
  container_name?: string;
  port?: number;
  env?: { name: string; value: string }[];
  resource_limits?: ResourceSpec;
  resource_requests?: ResourceSpec;
}

// --- AI Diagnostics types ---

export type DiagAgent = "claude" | "codex";

export type DiagStatus = "pending" | "running" | "succeeded" | "failed";

export interface DiagnoseRequest {
  pod_name: string;
  container?: string;
  logs: string;
  agent: DiagAgent;
}

export interface DiagnoseResponse {
  job_name: string;
  status: DiagStatus;
  agent: DiagAgent;
}

export interface DiagnosticStatusResponse {
  job_name: string;
  status: DiagStatus;
  agent: DiagAgent | null;
  source_pod: string | null;
  started_at: string | null;
  completed_at: string | null;
  message: string | null;
}

export interface DiagnosticResultResponse {
  job_name: string;
  status: DiagStatus;
  output: string;
}

// --- Templates ---

export type TemplateCategory =
  | "web_app"
  | "worker"
  | "cron_job"
  | "static_site";

export interface DeploymentTemplate {
  builtin?: boolean;
  id: string;
  name: string;
  description: string;
  icon: string;
  category: TemplateCategory;
  // Server returns a superset of CreateDeploymentRequest keys pre-filled.
  // We keep it as a Partial so extra hints (like a target port for a probe)
  // don't fail the type check.
  payload: Partial<CreateDeploymentRequest> & Record<string, unknown>;
}

export interface TemplateListResponse {
  templates: DeploymentTemplate[];
}

// --- Rollout history ---

export interface RevisionSummary {
  revision: number;
  replica_set_name: string;
  image: string;
  replicas: number;
  ready_replicas: number;
  created_at: string | null;
  change_cause: string | null;
  is_current: boolean;
}

export interface HistoryResponse {
  revisions: RevisionSummary[];
}

export interface RollbackRequest {
  revision: number;
}

// --- Validation ---

export interface ValidateResponse {
  ok: boolean;
  errors: string[];
}

// --- Clone ---

export interface CloneRequest {
  target_namespace: string;
  new_name?: string;
  overwrite?: boolean;
}

export interface CloneResponse extends DeploymentDetailResponse {
  source_namespace: string;
  source_name: string;
  target_namespace: string;
  target_name: string;
}


// --- AI Provider Config ---

export type AiProviderType = "native" | "vertex_ai" | "bedrock";

export interface AiProviderConfig {
  type: AiProviderType;
  api_key_secret?: string;
  project_id?: string;
  region?: string;
  sa_key_secret?: string;
  model_id?: string;
}

// --- Settings ---

export interface ResourceDefaults {
  cpu_request: string | null;
  memory_request: string | null;
  cpu_limit: string | null;
  memory_limit: string | null;
}

export interface AuthSettings {
  enabled: boolean;
  tenant_id: string;
  client_id: string;
}

export interface NotificationSettings {
  enabled: boolean;
  webhook_url: string;
}

export interface EncryptedCredentials {
  anthropic_api_key: string | null;
  gcp_sa_key: string | null;
}

export interface SetCredentialsRequest {
  anthropic_api_key?: string;
  gcp_sa_key?: string;
}

export interface SetCredentialsResponse {
  anthropic_api_key: string | null;
  gcp_sa_key: string | null;
}

export interface DeckwatchSettings {
  allowed_namespaces: string[];
  default_resource_limits: ResourceDefaults | null;
  auth: AuthSettings | null;
  notifications: NotificationSettings | null;
  git_repositories: GitRepository[];
  oci_registries: OciRegistry[];
  git_token_secrets: GitTokenSecret[];
  prometheus_enabled?: boolean;
  ai_claude_enabled?: boolean;
  ai_codex_enabled?: boolean;
  ai_provider?: AiProviderConfig;
  cost?: CostSettings | null;
  tracing?: TracingSettings | null;
  credentials?: EncryptedCredentials | null;
}

export interface CostSettings {
  cost_per_cpu_hour: number | null;
  cost_per_gb_hour: number | null;
  currency: string;
}

export interface TracingSettings {
  query_url: string;
  ui_url: string;
}

// --- Application types ---

export type ApplicationHealth = "healthy" | "degraded" | "unhealthy" | "empty";

export interface ApplicationGitConfig {
  repo_url: string;
  branch?: string;
  token_secret?: string;
}

export interface ApplicationSummary {
  name: string;
  namespace: string;
  description: string;
  created_at: string | null;
  deployment_count: number;
  cronjob_count: number;
  health: ApplicationHealth;
  gitops_enabled: boolean;
}

export interface ApplicationDetail {
  name: string;
  namespace: string;
  description: string;
  created_at: string | null;
  updated_at: string | null;
  git: ApplicationGitConfig | null;
  deployments: DeploymentSummary[];
  cronjobs: CronJobSummary[];
  health: ApplicationHealth;
}

export interface ApplicationListResponse {
  applications: ApplicationSummary[];
}

export interface CreateApplicationRequest {
  name: string;
  description?: string;
  git?: ApplicationGitConfig;
  create_deployment?: boolean;
  template_id?: string;
}

export interface UpdateApplicationRequest {
  description?: string;
  git?: ApplicationGitConfig;
}

export interface AddMemberRequest {
  kind: string;
  resource_name: string;
}

export interface UpdateAddonRequest {
  port?: number;
  env?: { name: string; value: string }[];
  resource_limits?: ResourceSpec;
  resource_requests?: ResourceSpec;
}

// --- GitOps Settings types ---

export type OciRegistryType = "ecr" | "dockerhub" | "ghcr" | "gar" | "harbor" | "generic" | "deckwatch";

export interface GitRepository {
  name: string;
  url: string;
  default_branch: string;
}

export interface OciRegistry {
  name: string;
  url: string;
  registry_type: OciRegistryType;
  builtin?: boolean;
}

export interface GitTokenSecret {
  name: string;
  secret_name: string;
  namespace: string;
}

export interface BranchListResponse {
  branches: string[];
  default_branch: string | null;
}

// --- Events ---

export interface EventSummary {
  namespace: string;
  name: string;
  event_type: string;
  reason: string | null;
  message: string | null;
  involved_object_kind: string;
  involved_object_name: string;
  involved_object_namespace: string | null;
  count: number | null;
  first_timestamp: string | null;
  last_timestamp: string | null;
  source_component: string | null;
  source_host: string | null;
}

export interface EventListResponse {
  events: EventSummary[];
}

// --- Secrets & ConfigMaps ---

export interface SecretSummary {
  name: string;
  namespace: string;
  secret_type: string;
  keys: string[];
  created_at: string | null;
}

export interface SecretDetail extends SecretSummary {
  data: Record<string, string>;
}

export interface SecretListResponse {
  secrets: SecretSummary[];
}

export interface CreateSecretRequest {
  name: string;
  data: Record<string, string>;
  secret_type?: string;
}

export interface ConfigMapSummary {
  name: string;
  namespace: string;
  keys: string[];
  created_at: string | null;
}

export interface ConfigMapDetail extends ConfigMapSummary {
  data: Record<string, string>;
}

export interface ConfigMapListResponse {
  configmaps: ConfigMapSummary[];
}

export interface CreateConfigMapRequest {
  name: string;
  data: Record<string, string>;
}

// --- Job Pods ---

export interface JobPodSummary {
  name: string;
  phase: string;
}

export interface JobPodListResponse {
  pods: JobPodSummary[];
}

// --- HPA Autoscaling ---

export interface HpaCondition {
  type: string;
  status: string;
  reason: string | null;
  message: string | null;
}

export interface HpaResponse {
  min_replicas: number;
  max_replicas: number;
  target_cpu_utilization: number | null;
  current_cpu_utilization: number | null;
  target_memory_utilization: number | null;
  current_memory_utilization: number | null;
  current_replicas: number;
  desired_replicas: number;
  conditions: HpaCondition[];
}

export interface HpaConfigRequest {
  min_replicas: number;
  max_replicas: number;
  target_cpu_utilization?: number;
  target_memory_utilization?: number;
}

// --- Notification Events ---

export type NotificationEventType =
  | "build_completed"
  | "build_failed"
  | "deployment_created"
  | "deployment_deleted"
  | "deployment_scaled"
  | "pod_crash_loop"
  | "application_created"
  | "application_deleted";

export interface DiagnosticHistoryItem {
  job_name: string;
  status: DiagStatus;
  agent: DiagAgent | null;
  source_pod: string | null;
  started_at: string | null;
  completed_at: string | null;
  created_at: string | null;
}

export interface DiagnosticHistoryResponse {
  items: DiagnosticHistoryItem[];
}

export interface TemplatesUpdateRequest {
  templates: DeploymentTemplate[];
}
