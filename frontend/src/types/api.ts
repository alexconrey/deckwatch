export interface DeploymentSummary {
  name: string;
  namespace: string;
  image: string;
  replicas: ReplicaCounts;
  status: DeploymentPhase;
  created_at: string | null;
  labels: Record<string, string>;
  resource_requests?: ResourceSpec | null;
}

export interface NodeSelectorRequirement {
  key: string;
  operator: "In" | "NotIn" | "Exists" | "DoesNotExist";
  values: string[];
}

export interface NodeSelectorTerm {
  match_expressions: NodeSelectorRequirement[];
}

export interface PreferredNodeTerm {
  weight: number;
  match_expressions: NodeSelectorRequirement[];
}

export interface NodeAffinityConfig {
  required: NodeSelectorTerm[];
  preferred: PreferredNodeTerm[];
}

export interface DeploymentDetail extends DeploymentSummary {
  annotations: Record<string, string>;
  conditions: DeploymentCondition[];
  env: EnvVar[];
  command: string[];
  args: string[];
  resource_limits: ResourceSpec | null;
  resource_requests: ResourceSpec | null;
  liveness_probe: ProbeConfig | null;
  readiness_probe: ProbeConfig | null;
  startup_probe: ProbeConfig | null;
  pod_labels: Record<string, string>;
  pod_annotations: Record<string, string>;
  node_selector: Record<string, string>;
  node_affinity: NodeAffinityConfig | null;
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
  oom_killed?: boolean;
}

export interface ContainerStatusSummary {
  name: string;
  ready: boolean;
  restart_count: number;
  state: string;
  state_reason: string | null;
  image: string;
  oom_killed?: boolean;
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
  pod_labels?: Record<string, string>;
  pod_annotations?: Record<string, string>;
  node_selector?: Record<string, string>;
  node_affinity?: NodeAffinityConfig;
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
  template?: string;
}

export interface IngressListResponse {
  ingresses: IngressSummary[];
}

export interface IngressTemplate {
  name: string;
  ingress_class: string | null;
  annotations: Record<string, string>;
  is_default: boolean;
}

// --- IngressClass types ---

export interface IngressClassParametersRef {
  api_group: string | null;
  kind: string;
  name: string;
  namespace: string | null;
  scope: string | null;
}

export interface IngressClassSummary {
  name: string;
  controller: string;
  is_default: boolean;
  parameters: IngressClassParametersRef | null;
}

export interface IngressClassListResponse {
  ingress_classes: IngressClassSummary[];
}

export interface CreateIngressClassRequest {
  name: string;
  controller: string;
  is_default?: boolean;
  parameters?: CreateIngressClassParametersRef | null;
}

export interface CreateIngressClassParametersRef {
  api_group?: string | null;
  kind: string;
  name: string;
  namespace?: string | null;
  scope?: string | null;
}

// --- GitOps types ---

export interface GitOpsConfig {
  repo_url: string;
  branch: string;
  token_secret: string;
  git_auth_user: string;
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
  git_auth_user?: string;
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

// --- StorageClass types ---

export interface StorageClassSummary {
  name: string;
  provisioner: string;
  reclaim_policy: string | null;
  volume_binding_mode: string | null;
  allow_volume_expansion: boolean;
  is_default: boolean;
  mount_options: string[] | null;
  parameters: Record<string, string> | null;
}

export interface StorageClassListResponse {
  storage_classes: StorageClassSummary[];
}

export interface CreateStorageClassRequest {
  name: string;
  provisioner: string;
  reclaim_policy?: string;
  volume_binding_mode?: string;
  allow_volume_expansion?: boolean;
  mount_options?: string[];
  parameters?: Record<string, string>;
  is_default?: boolean;
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
  storage?: string;
  storage_class?: string;
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
  redirect_uri?: string;
  scopes?: string;
}

export interface NotificationSettings {
  enabled: boolean;
  webhook_url: string;
  event_types?: string[];
  namespaces?: string[];
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

export type PluginSourceType = "github" | "url";

export interface PluginSourceGithub {
  type: "github";
  repo: string;
  ref: string;
  path: string;
  use_release: boolean;
}

export interface PluginSourceUrl {
  type: "url";
  url: string;
}

export type PluginSource = PluginSourceGithub | PluginSourceUrl;

export interface PluginConfig {
  name: string;
  enabled: boolean;
  source: PluginSource;
  token_secret?: string | null;
  /** Hosts the plugin can reach via extism's HTTP host function. */
  allowed_hosts: string[];
  /** Operator-supplied key-value config injected into the plugin's extism namespace. */
  config: Record<string, string>;
  /** Env var names to read from the deckwatch pod environment and inject into
   *  the plugin config at invocation time, overriding any same-named config entry.
   *  Use for credentials that are already mounted as pod env vars (e.g. from a
   *  Kubernetes Secret) so they don't need to be stored in settings. */
  inherit_env_keys: string[];
  /** Map of config_key → env_var_holding_file_path. Deckwatch reads the file
   *  and injects its content as the config key. Cloud-agnostic: the plugin
   *  decides what to do with the content (e.g. STS token exchange). */
  inherit_env_file_keys: Record<string, string>;
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
  default_storage_class?: string | null;
  ingress_templates?: IngressTemplate[];
  plugins?: PluginConfig[];
  mcp_tuning?: McpTuning;
  marketplace_url?: string;
}

export interface McpTuning {
  global?: string;
  namespaces?: string;
  deployments?: string;
  applications?: string;
  gitops?: string;
  ingresses?: string;
  pods?: string;
  secrets?: string;
  nodes?: string;
  storage?: string;
  plugins?: string;
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

export interface GitTokenSecretRequest {
  name: string;
  secret_name: string;
  token: string;
}

export interface GitTokenSecretResponse {
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

// --- ServiceAccounts ---

export interface ServiceAccountSummary {
  name: string;
  namespace: string;
  /** Populated when the IRSA annotation `eks.amazonaws.com/role-arn` is set. */
  irsa_role_arn: string | null;
  created_at: string | null;
  labels: Record<string, string>;
}

export interface ServiceAccountDetail extends ServiceAccountSummary {
  annotations: Record<string, string>;
}

export interface ServiceAccountListResponse {
  service_accounts: ServiceAccountSummary[];
}

export interface CreateServiceAccountRequest {
  name: string;
  annotations?: Record<string, string>;
  labels?: Record<string, string>;
}

export interface PatchServiceAccountRequest {
  annotations?: Record<string, string>;
  labels?: Record<string, string>;
}

// --- Job Pods ---

export interface JobPodSummary {
  name: string;
  phase: string;
  /** The Kubernetes Job that owns this pod. */
  job_name?: string;
  /** Build architecture (e.g. "amd64", "arm64", "manifest"). */
  arch?: string;
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

// --- Plugin schema types (Feature 1: self-describing plugin config) ---

export type ConfigFieldType = 'string' | 'secret' | 'bool' | 'select';

export interface ConfigField {
  key: string;
  label: string;
  description: string;
  field_type: ConfigFieldType;
  default?: string | null;
  required: boolean;
  options: string[];
  env_source?: string | null;
}

export interface PluginSummary {
  name: string;
  version: string;
  description: string;
  provides: string[];
  depends_on: string[];
  config_schema: ConfigField[];
  resources: PluginResource[];
  wasm_size_bytes: number;
}

// --- Application plugin association types (Feature 2) ---

export interface ApplicationPluginEntry {
  plugin_name: string;
  created_at: string;
  /** Whether the plugin is currently loaded in deckwatch. */
  is_loaded: boolean;
}

// --- Marketplace types ---

export type MarketplaceTrustLevel = 'verified' | 'community';

export interface MarketplaceEntry {
  name: string;
  slug: string;
  description: string;
  author: string;
  homepage: string;
  trust_level: MarketplaceTrustLevel;
  tags: string[];
  latest_version: string;
  source: PluginSource;
  allowed_hosts_hint: string[];
}

export interface MarketplaceCatalog {
  version: number;
  updated_at: string;
  plugins: MarketplaceEntry[];
}

// --- Plugin-declared provisioned resources ---

export interface PluginResource {
  id: string;
  label: string;
  icon: string;
  description: string;
  singleton: boolean;
  fields: ConfigField[];
  output_keys: string[];
}

export interface ProvisionedResource {
  id: string;
  application_id: string;
  plugin_name: string;
  resource_id: string;
  fields: Record<string, string>;
  state: Record<string, string>;
  created_at: string;
  updated_at: string;
}

export interface ProvisionRequest {
  fields: Record<string, string>;
}

// --- Preview environments ---

export interface CreatePreviewRequest {
  branch: string;
  pr_number?: number;
  host_suffix?: string;
  ttl_hours?: number;
}

export interface PreviewSummary {
  name: string;
  branch: string;
  pr_number: number | null;
  host: string | null;
  replicas_ready: number;
  replicas_desired: number;
  created_at: string | null;
  expires_at: string;
}

export interface PreviewListResponse {
  previews: PreviewSummary[];
}

// --- Promote ---

export interface PromoteRequest {
  target_namespace: string;
  target_name?: string;
  change_cause?: string;
}

export interface PromoteFieldChange {
  field: string;
  from: string | null | undefined;
  to: string | null | undefined;
}

export interface PromoteResponse {
  source_namespace: string;
  source_name: string;
  target_namespace: string;
  target_name: string;
  no_op: boolean;
  changes: PromoteFieldChange[];
}
