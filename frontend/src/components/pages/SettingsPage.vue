<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { gitTokensApi } from "@/api/gitTokens";
import { ingressclassesApi } from "@/api/ingressclasses";
import { pluginsApi } from "@/api/plugins";
import { settingsApi } from "@/api/settings";
import { storageclassesApi } from "@/api/storageclasses";
import { templatesApi } from "@/api/templates";
import { useAiSettings } from "@/composables/useAiSettings";
import { useClusterAlertSettings } from "@/composables/useClusterAlertSettings";
import AgentFeedbackPage from "@/components/pages/AgentFeedbackPage.vue";
import AuditLogPage from "@/components/pages/AuditLogPage.vue";
import MarketplacePage from "@/components/pages/MarketplacePage.vue";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import type {
  AiProviderConfig,
  AiProviderType,
  AuthSettings,
  CostSettings,
  CreateIngressClassRequest,
  CreateStorageClassRequest,
  DeckwatchSettings,
  DeploymentTemplate,
  EncryptedCredentials,
  GitRepository,
  GitTokenSecret,
  GitTokenSecretRequest,
  IngressClassSummary,
  IngressTemplate,
  McpTuning,
  McpTuningField,
  NotificationEventType,
  NotificationSettings,
  OciRegistry,
  OciRegistryType,
  PluginConfig,
  PluginSummary,
  ResourceDefaults,
  StorageClassSummary,
  TemplateCategory,
} from "@/types/api";

const router = useRouter();

const NOTIFICATION_EVENTS: { value: NotificationEventType; title: string; hint: string }[] = [
  { value: "build_completed", title: "Build succeeded", hint: "Fires when a kaniko job promotes a new image" },
  { value: "build_failed", title: "Build failed", hint: "Kaniko exited non-zero" },
  { value: "deployment_created", title: "Deployment created", hint: "New Deployment via the API" },
  { value: "deployment_deleted", title: "Deployment deleted", hint: "Deployment removed via the API" },
  { value: "deployment_scaled", title: "Deployment scaled", hint: "Replica count changed via the API" },
  { value: "pod_crash_loop", title: "Pod crash loop", hint: "CrashLoopBackOff detected (not yet wired)" },
  { value: "application_created", title: "Application created", hint: "New Application via the API" },
  { value: "application_deleted", title: "Application deleted", hint: "Application removed via the API" },
];

type SectionId =
  | "general"
  | "storage"
  | "networking"
  | "auth"
  | "ai_providers"
  | "observability"
  | "templates"
  | "git_repositories"
  | "container_registries"
  | "plugins"
  | "mcp_tuning"
  | "advanced"
  | "audit"
  | "agent_feedback";

const navItemsBase: { id: SectionId; title: string; icon: string }[] = [
  { id: "general", title: "General", icon: "mdi-tune" },
  { id: "storage", title: "Storage", icon: "mdi-database" },
  { id: "networking", title: "Networking", icon: "mdi-lan" },
  { id: "auth", title: "Authentication", icon: "mdi-shield-account" },
  { id: "ai_providers", title: "AI Providers", icon: "mdi-robot" },
  { id: "observability", title: "Observability", icon: "mdi-chart-line" },
  { id: "templates", title: "Templates", icon: "mdi-shape-outline" },
  { id: "git_repositories", title: "Git Repositories", icon: "mdi-git" },
  { id: "container_registries", title: "Container Registries", icon: "mdi-package-variant" },
  { id: "plugins", title: "Plugins", icon: "mdi-puzzle" },
  { id: "mcp_tuning", title: "MCP Tuning", icon: "mdi-brain" },
  { id: "advanced", title: "Advanced", icon: "mdi-cog-outline" },
  { id: "audit", title: "Audit Log", icon: "mdi-clipboard-text-clock" },
];

const navItems = computed(() => {
  if (!agentFeedbackEnabled.value) return navItemsBase;
  return [
    ...navItemsBase,
    { id: "agent_feedback" as SectionId, title: "Agent Feedback", icon: "mdi-message-text-outline" },
  ];
});

const section = ref<SectionId>("general");

const loading = ref(false);
const saving = ref(false);
const error = ref<string | null>(null);

const snackbar = ref(false);
const snackbarMessage = ref("");
const snackbarColor = ref<"success" | "error">("success");

const allowedNamespaces = ref<string[]>([]);
const resourceDefaults = ref<ResourceDefaults>({
  cpu_request: null,
  memory_request: null,
  cpu_limit: null,
  memory_limit: null,
});
const costSettings = ref<CostSettings>({
  cost_per_cpu_hour: null,
  cost_per_gb_hour: null,
  currency: "USD",
});
const auth = ref<AuthSettings>({
  enabled: false,
  tenant_id: "",
  client_id: "",
  redirect_uri: "",
  scopes: "openid profile email",
});
const notifications = ref<NotificationSettings>({
  enabled: false,
  webhook_url: "",
  event_types: [],
  namespaces: [],
});
const testingNotification = ref(false);

// Cluster warning-event toast notifications: browser-local toggle (stays
// per-browser since it's a personal preference, not a policy decision).
const { enabled: alertsEnabled } = useClusterAlertSettings();

// After saving settings, refresh the composable's cached copy so other
// components (DiagnoseButton, AiFixButton) pick up the new toggle state.
const { refresh: refreshAiSettings } = useAiSettings();

// Managed lists.
const prometheusEnabled = ref(true);
const agentFeedbackEnabled = ref(false);

// AI provider toggles are now server-side settings, persisted alongside the
// rest of DeckwatchSettings so an admin toggle applies to all users.
const aiClaudeEnabled = ref(true);
const aiCodexEnabled = ref(true);

const AI_PROVIDER_OPTIONS: { value: AiProviderType; title: string; description: string }[] = [
  { value: "native", title: "Anthropic (Native)", description: "Direct API via api.anthropic.com" },
  { value: "vertex_ai", title: "Google Vertex AI", description: "Anthropic models via GCP Vertex AI" },
  { value: "bedrock", title: "AWS Bedrock", description: "Anthropic models via AWS Bedrock (coming soon)" },
];

const aiProvider = ref<AiProviderConfig>({
  type: "native",
  api_key_secret: "deckwatch-anthropic-api-key",
});

// Encrypted credentials stored in the DB. The GET response returns
// "configured" (not the actual key) when a value is set, or null when empty.
const credentialStatus = ref<EncryptedCredentials>({
  anthropic_api_key: null,
  gcp_sa_key: null,
});
// Input fields for new credential values. These are never pre-filled --
// the user types a new key and clicks Save Credentials to encrypt + store.
const anthropicKeyInput = ref("");
const gcpSaKeyInput = ref("");
const savingCredentials = ref(false);

const storageClasses = ref<StorageClassSummary[]>([]);
const defaultStorageClass = ref<string | null>(null);

const scDialogOpen = ref(false);
const scDialogEditing = ref(false);
const scDialogSaving = ref(false);
const scForm = ref<CreateStorageClassRequest>({
  name: "",
  provisioner: "",
  reclaim_policy: "Delete",
  volume_binding_mode: "WaitForFirstConsumer",
  allow_volume_expansion: false,
  mount_options: [],
  parameters: {},
  is_default: false,
});
const scMountOptionsText = ref("");
const scParamRows = ref<{ key: string; value: string }[]>([]);

const scDeleteDialogOpen = ref(false);
const scDeleteTarget = ref("");
const scDeleting = ref(false);

function openCreateStorageClass() {
  scDialogEditing.value = false;
  scForm.value = {
    name: "",
    provisioner: "",
    reclaim_policy: "Delete",
    volume_binding_mode: "WaitForFirstConsumer",
    allow_volume_expansion: false,
    mount_options: [],
    parameters: {},
    is_default: false,
  };
  scMountOptionsText.value = "";
  scParamRows.value = [];
  scDialogOpen.value = true;
}

function openEditStorageClass(sc: StorageClassSummary) {
  scDialogEditing.value = true;
  scForm.value = {
    name: sc.name,
    provisioner: sc.provisioner,
    reclaim_policy: sc.reclaim_policy ?? "Delete",
    volume_binding_mode: sc.volume_binding_mode ?? "WaitForFirstConsumer",
    allow_volume_expansion: sc.allow_volume_expansion,
    mount_options: sc.mount_options ?? [],
    parameters: sc.parameters ?? {},
    is_default: sc.is_default,
  };
  scMountOptionsText.value = (sc.mount_options ?? []).join(", ");
  scParamRows.value = Object.entries(sc.parameters ?? {}).map(([key, value]) => ({ key, value }));
  scDialogOpen.value = true;
}

function addScParamRow() {
  scParamRows.value.push({ key: "", value: "" });
}

function removeScParamRow(idx: number) {
  scParamRows.value.splice(idx, 1);
}

async function saveStorageClass() {
  scDialogSaving.value = true;
  try {
    const mountOpts = scMountOptionsText.value
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    const params: Record<string, string> = {};
    for (const row of scParamRows.value) {
      if (row.key.trim()) params[row.key.trim()] = row.value;
    }
    const req: CreateStorageClassRequest = {
      ...scForm.value,
      mount_options: mountOpts.length > 0 ? mountOpts : undefined,
      parameters: Object.keys(params).length > 0 ? params : undefined,
    };
    if (scDialogEditing.value) {
      await storageclassesApi.update(req.name, req);
    } else {
      await storageclassesApi.create(req);
    }
    scDialogOpen.value = false;
    await loadStorageClasses();
    snackbarMessage.value = scDialogEditing.value ? "Storage class updated" : "Storage class created";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : "Failed to save storage class";
    snackbarMessage.value = msg;
    snackbarColor.value = "error";
    snackbar.value = true;
  } finally {
    scDialogSaving.value = false;
  }
}

function confirmDeleteStorageClass(name: string) {
  scDeleteTarget.value = name;
  scDeleteDialogOpen.value = true;
}

async function deleteStorageClass() {
  scDeleting.value = true;
  try {
    await storageclassesApi.delete(scDeleteTarget.value);
    scDeleteDialogOpen.value = false;
    await loadStorageClasses();
    snackbarMessage.value = "Storage class deleted";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : "Failed to delete storage class";
    snackbarMessage.value = msg;
    snackbarColor.value = "error";
    snackbar.value = true;
  } finally {
    scDeleting.value = false;
  }
}

const ingressClasses = ref<IngressClassSummary[]>([]);

const icDialogOpen = ref(false);
const icDialogEditing = ref(false);
const icDialogSaving = ref(false);
const icForm = ref<CreateIngressClassRequest>({
  name: "",
  controller: "",
  is_default: false,
  parameters: null,
});
const icShowParameters = ref(false);

const icDeleteDialogOpen = ref(false);
const icDeleteTarget = ref("");
const icDeleting = ref(false);

const loadIngressClasses = async () => {
  try {
    const res = await ingressclassesApi.list();
    ingressClasses.value = res.ingress_classes;
  } catch { /* silent */ }
};

function openCreateIngressClass() {
  icDialogEditing.value = false;
  icForm.value = {
    name: "",
    controller: "",
    is_default: false,
    parameters: null,
  };
  icShowParameters.value = false;
  icDialogOpen.value = true;
}

function openEditIngressClass(ic: IngressClassSummary) {
  icDialogEditing.value = true;
  icForm.value = {
    name: ic.name,
    controller: ic.controller,
    is_default: ic.is_default,
    parameters: ic.parameters
      ? {
          api_group: ic.parameters.api_group,
          kind: ic.parameters.kind,
          name: ic.parameters.name,
          namespace: ic.parameters.namespace,
          scope: ic.parameters.scope,
        }
      : null,
  };
  icShowParameters.value = !!ic.parameters;
  icDialogOpen.value = true;
}

async function saveIngressClass() {
  icDialogSaving.value = true;
  try {
    const req: CreateIngressClassRequest = {
      ...icForm.value,
      parameters: icShowParameters.value ? icForm.value.parameters : null,
    };
    if (icDialogEditing.value) {
      await ingressclassesApi.update(req.name, req);
    } else {
      await ingressclassesApi.create(req);
    }
    icDialogOpen.value = false;
    await loadIngressClasses();
    snackbarMessage.value = icDialogEditing.value ? "Ingress class updated" : "Ingress class created";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : "Failed to save ingress class";
    snackbarMessage.value = msg;
    snackbarColor.value = "error";
    snackbar.value = true;
  } finally {
    icDialogSaving.value = false;
  }
}

function confirmDeleteIngressClass(name: string) {
  icDeleteTarget.value = name;
  icDeleteDialogOpen.value = true;
}

async function deleteIngressClass() {
  icDeleting.value = true;
  try {
    await ingressclassesApi.delete(icDeleteTarget.value);
    icDeleteDialogOpen.value = false;
    await loadIngressClasses();
    snackbarMessage.value = "Ingress class deleted";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : "Failed to delete ingress class";
    snackbarMessage.value = msg;
    snackbarColor.value = "error";
    snackbar.value = true;
  } finally {
    icDeleting.value = false;
  }
}

const ingressTemplates = ref<IngressTemplate[]>([]);

const gitRepositories = ref<GitRepository[]>([]);

const plugins = ref<PluginConfig[]>([]);
// Loaded plugins from GET /api/plugins — drives the overview table in the plugins section.
const loadedPlugins = ref<PluginSummary[]>([]);
const loadedPluginsError = ref<string | null>(null);
const mcpTuning = ref<McpTuning>({});
const marketplaceUrl = ref('http://market.deckwatch.io/catalog.json');
const marketplaceOpen = ref<string | null>(null);

const mcpTuningGroups = [
  {
    key: "namespaces", label: "Namespaces", icon: "mdi-view-grid-outline",
    examples: ["create_namespace", "list_namespaces", "create_application"],
    placeholder: "e.g. Use team-based namespaces (team-platform, team-rad). Never create per-app namespaces.",
  },
  {
    key: "deployments", label: "Deployments", icon: "mdi-rocket-launch-outline",
    examples: ["create_deployment", "update_deployment", "scale_deployment"],
    placeholder: "e.g. Always set resource requests and limits. Production deployments require readiness probes.",
  },
  {
    key: "applications", label: "Applications & Addons", icon: "mdi-application-outline",
    examples: ["create_application", "attach_addon", "list_templates"],
    placeholder: "e.g. Use the web-app template for all HTTP services. Attach Redis only for stateful workloads.",
  },
  {
    key: "gitops", label: "GitOps & Builds", icon: "mdi-source-branch",
    examples: ["set_gitops", "trigger_build", "list_builds"],
    placeholder: "e.g. All production images must be built from the main branch, never from feature branches.",
  },
  {
    key: "ingresses", label: "Ingresses", icon: "mdi-transit-connection-variant",
    examples: ["create_ingress", "update_ingress", "list_ingress_templates"],
    placeholder: "e.g. Use the internal-alb template for all ingresses. Never expose services publicly.",
  },
  {
    key: "pods", label: "Pods", icon: "mdi-cube-outline",
    examples: ["list_pods", "get_pod_logs", "exec_pod"],
    placeholder: "e.g. Pod exec access is restricted to the platform team. Use log streaming for debugging.",
  },
  {
    key: "secrets", label: "Secrets", icon: "mdi-lock-outline",
    examples: ["list_secrets", "get_secret", "create_secret"],
    placeholder: "e.g. Secrets must be created via ExternalSecrets from Vault, not manually via kubectl.",
  },
  {
    key: "nodes", label: "Nodes", icon: "mdi-server-outline",
    examples: ["list_nodes", "cordon_node", "drain_node"],
    placeholder: "e.g. Node maintenance requires a change request. Always drain before cordoning.",
  },
  {
    key: "storage", label: "Storage", icon: "mdi-database-outline",
    examples: ["list_pvcs", "create_pvc", "list_storageclasses"],
    placeholder: "e.g. Use the gp3 storage class for all persistent volumes.",
  },
  {
    key: "plugins", label: "Plugins", icon: "mdi-puzzle-outline",
    examples: ["list_plugins", "enable_plugin", "validate_plugin"],
    placeholder: "e.g. Always validate a plugin before enabling it in production.",
  },
] as const;
const ociRegistries = ref<OciRegistry[]>([]);
const gitTokenSecrets = ref<GitTokenSecret[]>([]);

const REGISTRY_TYPES: { value: OciRegistryType; title: string; icon: string }[] = [
  { value: "ecr", title: "Amazon ECR", icon: "mdi-aws" },
  { value: "dockerhub", title: "Docker Hub", icon: "mdi-docker" },
  { value: "ghcr", title: "GitHub Container Registry", icon: "mdi-github" },
  { value: "gar", title: "Google Artifact Registry", icon: "mdi-google-cloud" },
  { value: "harbor", title: "Harbor", icon: "mdi-anchor" },
  { value: "generic", title: "Generic OCI", icon: "mdi-package-variant" },
];

const TEMPLATE_CATEGORIES: { value: TemplateCategory; title: string }[] = [
  { value: "web_app", title: "Web App" },
  { value: "worker", title: "Worker" },
  { value: "cron_job", title: "Cron Job" },
  { value: "static_site", title: "Static Site" },
];

// The deployment templates list is fetched separately from DeckwatchSettings —
// they live in their own ConfigMap so a broken template edit can't wedge the
// (much more critical) settings load path.
const templates = ref<DeploymentTemplate[]>([]);
// Snapshot of the compiled-in defaults, keyed by id. Populated from the
// initial GET before any user edits, so "Reset to Default" can restore
// a builtin entry without a round-trip.
const defaultTemplates = ref<Map<string, DeploymentTemplate>>(new Map());
const savingTemplates = ref(false);

const nextCustomTemplateId = computed(() => {
  const existing = new Set(templates.value.map((t) => t.id));
  let i = 1;
  while (existing.has(`custom-${i}`)) i++;
  return `custom-${i}`;
});

function applySettings(s: DeckwatchSettings) {
  allowedNamespaces.value = s.allowed_namespaces ?? [];
  resourceDefaults.value = s.default_resource_limits ?? {
    cpu_request: null,
    memory_request: null,
    cpu_limit: null,
    memory_limit: null,
  };
  costSettings.value = s.cost ?? {
    cost_per_cpu_hour: null,
    cost_per_gb_hour: null,
    currency: "USD",
  };
  if (s.auth) {
    auth.value = {
      enabled: s.auth.enabled,
      tenant_id: s.auth.tenant_id ?? "",
      client_id: s.auth.client_id ?? "",
      redirect_uri: s.auth.redirect_uri ?? "",
      scopes: s.auth.scopes ?? "openid profile email",
    };
  }
  if (s.notifications) {
    notifications.value = {
      enabled: s.notifications.enabled,
      webhook_url: s.notifications.webhook_url ?? "",
      event_types: s.notifications.event_types ?? [],
      namespaces: s.notifications.namespaces ?? [],
    };
  }
  gitRepositories.value = s.git_repositories ?? [];
  plugins.value = s.plugins ?? [];
  mcpTuning.value = s.mcp_tuning ?? {};
  marketplaceUrl.value = s.marketplace_url ?? 'http://market.deckwatch.io/catalog.json';
  ociRegistries.value = s.oci_registries ?? [];
  gitTokenSecrets.value = s.git_token_secrets ?? [];
  ingressTemplates.value = (s.ingress_templates ?? []).map((t) => ({
    ...t,
    annotations: { ...t.annotations },
  }));
  prometheusEnabled.value = s.prometheus_enabled ?? true;
  aiClaudeEnabled.value = s.ai_claude_enabled ?? true;
  aiCodexEnabled.value = s.ai_codex_enabled ?? true;
  agentFeedbackEnabled.value = s.agent_feedback_enabled ?? false;
  aiProvider.value = s.ai_provider ?? {
    type: "native",
    api_key_secret: "deckwatch-anthropic-api-key",
  };
  credentialStatus.value = s.credentials ?? {
    anthropic_api_key: null,
    gcp_sa_key: null,
  };
  defaultStorageClass.value = s.default_storage_class ?? null;
}

const loadStorageClasses = async () => {
  try {
    const res = await storageclassesApi.list();
    storageClasses.value = res.storage_classes;
  } catch { /* silent */ }
};

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const [s, t] = await Promise.all([
      settingsApi.get(),
      templatesApi.list(),
      loadStorageClasses(),
      loadIngressClasses(),
    ]);
    applySettings(s);
    applyTemplates(t.templates);
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to load settings";
  } finally {
    loading.value = false;
  }
  // Fetch loaded plugins separately so a plugin-API failure doesn't block
  // the critical settings load path.
  try {
    loadedPlugins.value = await pluginsApi.list();
    loadedPluginsError.value = null;
  } catch {
    loadedPluginsError.value = "Could not load plugin list from deckwatch.";
  }
}

function applyTemplates(list: DeploymentTemplate[]) {
  templates.value = list.map(cloneTemplate);
  // First load populates the defaults snapshot; subsequent loads keep the
  // original defaults so "Reset" still works even after saving (the returned
  // list reflects the user's overrides for builtin entries).
  if (defaultTemplates.value.size === 0) {
    for (const t of list) {
      if (t.builtin) {
        defaultTemplates.value.set(t.id, cloneTemplate(t));
      }
    }
  }
}

function cloneTemplate(t: DeploymentTemplate): DeploymentTemplate {
  return {
    ...t,
    payload: JSON.parse(JSON.stringify(t.payload ?? {})),
  };
}

function buildPayload(): DeckwatchSettings {
  return {
    allowed_namespaces: allowedNamespaces.value,
    default_resource_limits: hasAnyResourceDefault()
      ? resourceDefaults.value
      : null,
    auth: auth.value,
    notifications: notifications.value,
    git_repositories: gitRepositories.value,
    oci_registries: ociRegistries.value,
    git_token_secrets: gitTokenSecrets.value,
    cost: hasAnyCostRate() ? costSettings.value : null,
    prometheus_enabled: prometheusEnabled.value,
    ai_claude_enabled: aiClaudeEnabled.value,
    ai_codex_enabled: aiCodexEnabled.value,
    agent_feedback_enabled: agentFeedbackEnabled.value,
    ai_provider: aiProvider.value,
    default_storage_class: defaultStorageClass.value || null,
    ingress_templates: ingressTemplates.value,
    plugins: plugins.value,
    mcp_tuning: mcpTuning.value,
    marketplace_url: marketplaceUrl.value,
  };
}

function hasAnyResourceDefault(): boolean {
  const r = resourceDefaults.value;
  return !!(r.cpu_request || r.memory_request || r.cpu_limit || r.memory_limit);
}

// Persist the cost object only when at least one rate is set — an empty
// object would still enable the overlay on the frontend but render nothing,
// which is worse than the null state.
function hasAnyCostRate(): boolean {
  const c = costSettings.value;
  return c.cost_per_cpu_hour !== null || c.cost_per_gb_hour !== null;
}

function validateManagedLists(): string | null {
  const seen = new Set<string>();
  for (const r of gitRepositories.value) {
    if (!r.name || !r.url) return "Every Git repository needs a name and URL.";
    if (seen.has(`repo:${r.name}`)) return `Duplicate repository name: ${r.name}`;
    seen.add(`repo:${r.name}`);
  }
  for (const r of ociRegistries.value) {
    if (!r.name || !r.url) return "Every OCI registry needs a name and URL.";
    if (seen.has(`reg:${r.name}`)) return `Duplicate registry name: ${r.name}`;
    seen.add(`reg:${r.name}`);
  }
  for (const t of gitTokenSecrets.value) {
    if (!t.name || !t.secret_name) {
      return "Every Git token needs a display name and secret name.";
    }
    if (seen.has(`tok:${t.name}`)) return `Duplicate token name: ${t.name}`;
    seen.add(`tok:${t.name}`);
  }
  let defaultCount = 0;
  for (const it of ingressTemplates.value) {
    if (!it.name) return "Every ingress template needs a name.";
    if (seen.has(`itpl:${it.name}`)) return `Duplicate ingress template name: ${it.name}`;
    seen.add(`itpl:${it.name}`);
    if (it.is_default) defaultCount++;
  }
  if (defaultCount > 1) return "Only one ingress template can be marked as default.";
  return null;
}

async function save() {
  const validationError = validateManagedLists() ?? validateTemplates();
  if (validationError) {
    error.value = validationError;
    snackbarMessage.value = validationError;
    snackbarColor.value = "error";
    snackbar.value = true;
    return;
  }
  saving.value = true;
  error.value = null;
  try {
    // Order matters only for the snackbar wording — if templates fail after
    // settings succeed, we still want the user to see the templates error.
    const updated = await settingsApi.update(buildPayload());
    applySettings(updated);
    // Refresh the module-cached AI toggles so DiagnoseButton / AiFixButton
    // pick up the new enabled state without a page reload.
    void refreshAiSettings();
    const updatedTemplates = await templatesApi.update(templates.value);
    applyTemplates(updatedTemplates.templates);
    snackbarMessage.value = "Settings saved";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : "Failed to save settings";
    error.value = msg;
    snackbarMessage.value = msg;
    snackbarColor.value = "error";
    snackbar.value = true;
  } finally {
    saving.value = false;
  }
}

function validateTemplates(): string | null {
  const seen = new Set<string>();
  for (const t of templates.value) {
    if (!t.id?.trim()) return "Every template needs an id.";
    if (!t.name?.trim()) return `Template "${t.id}" needs a display name.`;
    if (seen.has(t.id)) return `Duplicate template id: ${t.id}`;
    seen.add(t.id);
    if (typeof t.payload !== "object" || t.payload === null) {
      return `Template "${t.id}" payload must be an object.`;
    }
  }
  return null;
}

async function testNotification() {
  // The backend reads settings from the ConfigMap at send time, so the URL
  // must be persisted first. Save unsaved edits before firing so the operator
  // isn't testing an old URL.
  testingNotification.value = true;
  try {
    await settingsApi.update(buildPayload());
    await settingsApi.testNotification();
    snackbarMessage.value = "Test notification sent";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : "Test notification failed";
    snackbarMessage.value = msg;
    snackbarColor.value = "error";
    snackbar.value = true;
  } finally {
    testingNotification.value = false;
  }
}

async function saveAdvanced() {
  try {
    const updated = await settingsApi.update(buildPayload());
    applySettings(updated);
    snackbarMessage.value = "Advanced settings saved";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e: any) {
    snackbarMessage.value = e.message ?? "Failed to save";
    snackbarColor.value = "error";
    snackbar.value = true;
  }
}

async function saveCredentials() {
  savingCredentials.value = true;
  try {
    const req: Record<string, string> = {};
    if (anthropicKeyInput.value) {
      req.anthropic_api_key = anthropicKeyInput.value;
    }
    if (gcpSaKeyInput.value) {
      req.gcp_sa_key = gcpSaKeyInput.value;
    }
    if (Object.keys(req).length === 0) {
      snackbarMessage.value = "No credentials to save";
      snackbarColor.value = "error";
      snackbar.value = true;
      return;
    }
    const result = await settingsApi.setCredentials(req);
    credentialStatus.value = {
      anthropic_api_key: result.anthropic_api_key,
      gcp_sa_key: result.gcp_sa_key,
    };
    // Clear inputs after successful save.
    anthropicKeyInput.value = "";
    gcpSaKeyInput.value = "";
    snackbarMessage.value = "Credentials saved and encrypted";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : "Failed to save credentials";
    snackbarMessage.value = msg;
    snackbarColor.value = "error";
    snackbar.value = true;
  } finally {
    savingCredentials.value = false;
  }
}

async function clearCredential(key: "anthropic_api_key" | "gcp_sa_key") {
  savingCredentials.value = true;
  try {
    const req: Record<string, string> = { [key]: "" };
    const result = await settingsApi.setCredentials(req);
    credentialStatus.value = {
      anthropic_api_key: result.anthropic_api_key,
      gcp_sa_key: result.gcp_sa_key,
    };
    snackbarMessage.value = "Credential removed";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : "Failed to clear credential";
    snackbarMessage.value = msg;
    snackbarColor.value = "error";
    snackbar.value = true;
  } finally {
    savingCredentials.value = false;
  }
}

// --- managed list helpers ---

function addRepository() {
  gitRepositories.value.push({ name: "", url: "", default_branch: "main" });
}
function removeRepository(idx: number) {
  gitRepositories.value.splice(idx, 1);
}

function addRegistry() {
  ociRegistries.value.push({ name: "", url: "", registry_type: "generic" });
}
function removeRegistry(idx: number) {
  ociRegistries.value.splice(idx, 1);
}

// --- Git token dialog state ---

const tokenDialogOpen = ref(false);
const tokenDialogMode = ref<"create" | "update">("create");
const tokenDialogSaving = ref(false);
const tokenForm = ref<GitTokenSecretRequest>({ name: "", secret_name: "", token: "" });

const tokenDeleteDialogOpen = ref(false);
const tokenDeleteTarget = ref("");
const tokenDeleting = ref(false);

function openAddTokenDialog() {
  tokenDialogMode.value = "create";
  tokenForm.value = { name: "", secret_name: "", token: "" };
  tokenDialogOpen.value = true;
}

function openUpdateTokenDialog(t: GitTokenSecret) {
  tokenDialogMode.value = "update";
  tokenForm.value = { name: t.name, secret_name: t.secret_name, token: "" };
  tokenDialogOpen.value = true;
}

function autoGenerateSecretName() {
  if (tokenDialogMode.value === "create" && !tokenForm.value.secret_name) {
    const slug = tokenForm.value.name
      .toLowerCase()
      .replace(/[^a-z0-9-]/g, "-")
      .replace(/-+/g, "-")
      .replace(/^-|-$/g, "");
    if (slug) {
      tokenForm.value.secret_name = `${slug}-token`;
    }
  }
}

async function saveTokenSecret() {
  autoGenerateSecretName();
  const needsSecretName = tokenDialogMode.value === "create";
  if (!tokenForm.value.name || (needsSecretName && !tokenForm.value.secret_name) || !tokenForm.value.token) return;
  tokenDialogSaving.value = true;
  try {
    const result = await gitTokensApi.upsert(tokenForm.value);
    const existing = gitTokenSecrets.value.findIndex(
      (t) => t.secret_name === result.secret_name,
    );
    if (existing >= 0) {
      gitTokenSecrets.value[existing] = {
        name: result.name,
        secret_name: result.secret_name,
        namespace: result.namespace,
      };
    } else {
      gitTokenSecrets.value.push({
        name: result.name,
        secret_name: result.secret_name,
        namespace: result.namespace,
      });
    }
    tokenDialogOpen.value = false;
    snackbarMessage.value = tokenDialogMode.value === "create"
      ? "Git token created"
      : "Git token updated";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : "Failed to save git token";
    snackbarMessage.value = msg;
    snackbarColor.value = "error";
    snackbar.value = true;
  } finally {
    tokenDialogSaving.value = false;
  }
}

function confirmDeleteToken(secretName: string) {
  tokenDeleteTarget.value = secretName;
  tokenDeleteDialogOpen.value = true;
}

async function deleteTokenSecret() {
  tokenDeleting.value = true;
  try {
    await gitTokensApi.remove(tokenDeleteTarget.value);
    gitTokenSecrets.value = gitTokenSecrets.value.filter(
      (t) => t.secret_name !== tokenDeleteTarget.value,
    );
    tokenDeleteDialogOpen.value = false;
    snackbarMessage.value = "Git token deleted";
    snackbarColor.value = "success";
    snackbar.value = true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : "Failed to delete git token";
    snackbarMessage.value = msg;
    snackbarColor.value = "error";
    snackbar.value = true;
  } finally {
    tokenDeleting.value = false;
  }
}

function addIngressTemplate() {
  ingressTemplates.value.push({
    name: "",
    ingress_class: null,
    annotations: {},
    is_default: false,
  });
}
function removeIngressTemplate(idx: number) {
  ingressTemplates.value.splice(idx, 1);
}
function addIngressTemplateAnnotation(idx: number) {
  const tpl = ingressTemplates.value[idx];
  if (!tpl) return;
  tpl.annotations = { ...tpl.annotations, "": "" };
}
function removeIngressTemplateAnnotation(idx: number, key: string) {
  const tpl = ingressTemplates.value[idx];
  if (!tpl) return;
  const next = { ...tpl.annotations };
  delete next[key];
  tpl.annotations = next;
}
function updateIngressTemplateAnnotationKey(
  tplIdx: number,
  oldKey: string,
  newKey: string,
) {
  const tpl = ingressTemplates.value[tplIdx];
  if (!tpl) return;
  const value = tpl.annotations[oldKey] ?? "";
  const next = { ...tpl.annotations };
  delete next[oldKey];
  next[newKey] = value;
  tpl.annotations = next;
}
function updateIngressTemplateAnnotationValue(
  tplIdx: number,
  key: string,
  value: string,
) {
  const tpl = ingressTemplates.value[tplIdx];
  if (!tpl) return;
  tpl.annotations = { ...tpl.annotations, [key]: value };
}
function setIngressTemplateDefault(idx: number) {
  for (let i = 0; i < ingressTemplates.value.length; i++) {
    ingressTemplates.value[i].is_default = i === idx;
  }
}

// --- plugin helpers ---

function addPlugin() {
  plugins.value.push({
    name: "",
    enabled: true,
    source: { type: "github", repo: "", ref: "main", path: "plugin.wasm", use_release: false },
    token_secret: null,
    allowed_hosts: [],
    config: {},
    inherit_env_keys: [],
    inherit_env_file_keys: {},
    mcp_tuning: {},
  });
}

function setPluginMcpTuning(pluginName: string, fieldKey: string, value: string) {
  const idx = plugins.value.findIndex((p) => p.name === pluginName);
  if (idx === -1) return;
  const plugin = plugins.value[idx];
  plugins.value[idx] = {
    ...plugin,
    mcp_tuning: { ...(plugin.mcp_tuning ?? {}), [fieldKey]: value },
  };
}

function removePlugin(idx: number) {
  plugins.value.splice(idx, 1);
}

// --- template helpers ---

function addTemplate() {
  const id = nextCustomTemplateId.value;
  templates.value.push({
    id,
    name: "Custom Template",
    description: "",
    icon: "mdi-cube-outline",
    category: "web_app",
    payload: {
      name: "",
      image: "",
      replicas: 1,
    },
    builtin: false,
  });
}

function removeTemplate(idx: number) {
  templates.value.splice(idx, 1);
}

function resetTemplate(idx: number) {
  const current = templates.value[idx];
  if (!current) return;
  const original = defaultTemplates.value.get(current.id);
  if (!original) return;
  templates.value.splice(idx, 1, cloneTemplate(original));
}

// The payload is edited as JSON so operators can access every field the
// deployment API accepts (including probes, ports, and cmd/args) without
// building a dedicated form for each. We stringify on read and parse on
// blur so the textarea binds to a plain string.
function stringifyPayload(payload: unknown): string {
  try {
    return JSON.stringify(payload ?? {}, null, 2);
  } catch {
    return "{}";
  }
}

function updatePayloadFromString(idx: number, raw: string) {
  try {
    const parsed = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      throw new Error("payload must be a JSON object");
    }
    templates.value[idx].payload = parsed;
  } catch (e) {
    error.value = `Template "${templates.value[idx]?.id}" payload: ${e instanceof Error ? e.message : "invalid JSON"}`;
  }
}

// Convenience getters/setters for the common payload fields so we can edit
// them with plain <v-text-field>s instead of forcing operators into raw JSON.
function payloadField<T>(idx: number, key: string): T | undefined {
  return (templates.value[idx]?.payload as Record<string, unknown>)?.[key] as T | undefined;
}
function setPayloadField(idx: number, key: string, value: unknown) {
  const p = templates.value[idx]?.payload as Record<string, unknown> | undefined;
  if (!p) return;
  if (value === "" || value === undefined || value === null) {
    delete p[key];
  } else {
    p[key] = value;
  }
}

onMounted(load);
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <h2 class="text-h5">Settings</h2>
      <v-spacer />
      <v-btn
        v-if="section !== 'audit'"
        color="primary"
        prepend-icon="mdi-content-save"
        :loading="saving"
        :disabled="loading"
        @click="save"
      >
        Save
      </v-btn>
    </div>

    <v-alert v-if="error" type="error" class="mb-4" closable>
      {{ error }}
    </v-alert>

    <div class="d-flex" style="gap: 16px">
      <!-- Sidebar navigation -->
      <v-card class="bg-surface flex-shrink-0" flat style="min-width: 220px; max-width: 240px">
        <v-list density="comfortable" nav>
          <v-list-item
            v-for="item in navItems"
            :key="item.id"
            :prepend-icon="item.icon"
            :title="item.title"
            :active="section === item.id"
            color="primary"
            @click="section = item.id"
          />
        </v-list>
      </v-card>

      <!-- Content panel -->
      <v-card class="bg-surface flex-grow-1 pa-6" flat>
        <!-- General -->
        <div v-if="section === 'general'">
          <div v-if="loading" class="d-flex justify-center pa-8">
            <v-progress-circular indeterminate color="primary" />
          </div>
          <template v-else>
            <h3 class="text-h6 mb-2">Allowed namespaces</h3>
            <p class="text-body-2 text-secondary mb-3">
              Restrict which namespaces deckwatch can view or modify. Leave
              empty to allow all cluster namespaces.
            </p>
            <v-combobox
              v-model="allowedNamespaces"
              label="Namespaces"
              multiple
              chips
              closable-chips
              variant="outlined"
              density="comfortable"
              hint="Press Enter to add a namespace"
              persistent-hint
              class="mb-6"
            />

            <v-divider class="mb-6" />

            <h3 class="text-h6 mb-2">Default resource limits</h3>
            <p class="text-body-2 text-secondary mb-3">
              Applied to new deployments created through deckwatch when the
              user does not override them.
            </p>
            <v-row>
              <v-col cols="12" md="6">
                <v-text-field
                  v-model="resourceDefaults.cpu_request"
                  label="CPU request"
                  placeholder="100m"
                  variant="outlined"
                  density="comfortable"
                />
              </v-col>
              <v-col cols="12" md="6">
                <v-text-field
                  v-model="resourceDefaults.memory_request"
                  label="Memory request"
                  placeholder="128Mi"
                  variant="outlined"
                  density="comfortable"
                />
              </v-col>
              <v-col cols="12" md="6">
                <v-text-field
                  v-model="resourceDefaults.cpu_limit"
                  label="CPU limit"
                  placeholder="500m"
                  variant="outlined"
                  density="comfortable"
                />
              </v-col>
              <v-col cols="12" md="6">
                <v-text-field
                  v-model="resourceDefaults.memory_limit"
                  label="Memory limit"
                  placeholder="512Mi"
                  variant="outlined"
                  density="comfortable"
                />
              </v-col>
            </v-row>

            <v-divider class="my-6" />

            <h3 class="text-h6 mb-2">Cost overlay</h3>
            <p class="text-body-2 text-secondary mb-3">
              When at least one rate is set, deckwatch renders per-hour and
              per-month cost estimates alongside deployment resource fields
              and warns when an edit more than doubles the current cost.
              Rates are per running unit-hour; replicas multiply at render
              time. Leave both blank to hide the overlay entirely.
            </p>
            <v-row>
              <v-col cols="12" md="4">
                <v-text-field
                  v-model.number="costSettings.cost_per_cpu_hour"
                  label="Cost per vCPU-hour"
                  placeholder="0.048"
                  type="number"
                  step="0.001"
                  min="0"
                  variant="outlined"
                  density="comfortable"
                  hint="e.g. m5.large on-demand blend"
                  persistent-hint
                />
              </v-col>
              <v-col cols="12" md="4">
                <v-text-field
                  v-model.number="costSettings.cost_per_gb_hour"
                  label="Cost per GiB-hour"
                  placeholder="0.006"
                  type="number"
                  step="0.001"
                  min="0"
                  variant="outlined"
                  density="comfortable"
                  hint="Memory pricing per gibibyte-hour"
                  persistent-hint
                />
              </v-col>
              <v-col cols="12" md="4">
                <v-text-field
                  v-model="costSettings.currency"
                  label="Currency"
                  placeholder="USD"
                  variant="outlined"
                  density="comfortable"
                  hint="ISO 4217 code; symbol chosen for USD/EUR/GBP/JPY"
                  persistent-hint
                />
              </v-col>
            </v-row>


            <v-divider class="my-6" />

            <h3 class="text-h6 mb-2">Notifications</h3>
            <p class="text-body-2 text-secondary mb-4">
              Deckwatch fires JSON POSTs to a single webhook URL when the
              events below occur. The payload is Slack-compatible (a top-level
              <code>text</code> plus a colored <code>attachments</code> block)
              but also works with Microsoft Teams incoming webhooks and any
              generic JSON receiver.
            </p>

            <v-switch
              v-model="notifications.enabled"
              color="primary"
              label="Enable webhook notifications"
              hide-details
              class="mb-4"
            />

            <v-text-field
              v-model="notifications.webhook_url"
              label="Webhook URL"
              placeholder="https://hooks.slack.com/services/T00/B00/xxx"
              variant="outlined"
              density="comfortable"
              class="mb-4"
            />

            <v-divider class="mb-4" />

            <h4 class="text-subtitle-1 mb-2">Namespaces</h4>
            <p class="text-body-2 text-secondary mb-3">
              Restrict which namespaces trigger notifications. Leave empty to
              fire for every allowed namespace.
            </p>
            <v-combobox
              v-model="notifications.namespaces"
              label="Namespaces"
              multiple
              chips
              closable-chips
              variant="outlined"
              density="comfortable"
              hint="Press Enter to add a namespace"
              persistent-hint
              class="mb-4"
            />

            <v-divider class="mb-4" />

            <h4 class="text-subtitle-1 mb-2">Event types</h4>
            <p class="text-body-2 text-secondary mb-3">
              Uncheck to mute a class of event. If nothing is checked, deckwatch
              treats it as "everything" to match the pre-filter behavior.
            </p>
            <v-row dense>
              <v-col
                v-for="evt in NOTIFICATION_EVENTS"
                :key="evt.value"
                cols="12"
                md="6"
              >
                <v-checkbox
                  v-model="notifications.event_types"
                  :label="evt.title"
                  :value="evt.value"
                  :hint="evt.hint"
                  persistent-hint
                  density="comfortable"
                />
              </v-col>
            </v-row>

            <v-divider class="my-4" />

            <div class="d-flex align-center">
              <v-btn
                color="secondary"
                variant="tonal"
                prepend-icon="mdi-send-outline"
                :loading="testingNotification"
                :disabled="!notifications.enabled || !notifications.webhook_url"
                @click="testNotification"
              >
                Send test notification
              </v-btn>
              <span class="text-caption text-secondary ml-3">
                Saves settings, then POSTs a `test` event.
              </span>
            </div>

          </template>
        </div>

        <!-- Storage -->
        <div v-else-if="section === 'storage'">
          <h3 class="text-h6 mb-2">Default storage class</h3>
          <p class="text-body-2 text-secondary mb-3">
            When set, addons that provision persistent volumes (e.g. PostgreSQL)
            will use this storage class instead of the cluster default. Leave as
            "None" to let the cluster decide.
          </p>

          <v-select
            v-model="defaultStorageClass"
            :items="[
              { title: 'None (cluster default)', value: '' },
              ...storageClasses.map((sc) => ({ title: sc.name, value: sc.name })),
            ]"
            item-title="title"
            item-value="value"
            label="Default Storage Class"
            variant="outlined"
            density="comfortable"
            class="mb-6"
          />

          <v-divider class="mb-6" />

          <div class="d-flex align-center mb-2">
            <h3 class="text-h6">Storage classes</h3>
            <v-spacer />
            <v-btn
              size="small"
              color="primary"
              variant="tonal"
              prepend-icon="mdi-plus"
              @click="openCreateStorageClass"
            >
              Create Storage Class
            </v-btn>
          </div>
          <p class="text-body-2 text-secondary mb-3">
            Manage StorageClass resources in the cluster.
          </p>

          <div v-if="storageClasses.length === 0" class="text-center py-6 text-secondary">
            No storage classes found in the cluster.
          </div>

          <v-table v-else density="comfortable">
            <thead>
              <tr>
                <th>Name</th>
                <th>Provisioner</th>
                <th>Reclaim Policy</th>
                <th>Volume Binding Mode</th>
                <th>Allow Expansion</th>
                <th>Default</th>
                <th class="text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="sc in storageClasses" :key="sc.name">
                <td>{{ sc.name }}</td>
                <td>{{ sc.provisioner }}</td>
                <td>{{ sc.reclaim_policy ?? "-" }}</td>
                <td>{{ sc.volume_binding_mode ?? "-" }}</td>
                <td>
                  <v-icon
                    :icon="sc.allow_volume_expansion ? 'mdi-check-circle' : 'mdi-close-circle'"
                    :color="sc.allow_volume_expansion ? 'success' : 'grey'"
                    size="small"
                  />
                </td>
                <td>
                  <v-chip
                    v-if="sc.is_default"
                    size="x-small"
                    color="primary"
                    variant="tonal"
                  >
                    default
                  </v-chip>
                </td>
                <td class="text-right">
                  <v-btn
                    icon="mdi-pencil"
                    variant="text"
                    size="small"
                    @click="openEditStorageClass(sc)"
                  />
                  <v-btn
                    icon="mdi-delete"
                    variant="text"
                    color="error"
                    size="small"
                    @click="confirmDeleteStorageClass(sc.name)"
                  />
                </td>
              </tr>
            </tbody>
          </v-table>

          <!-- Create / Edit Storage Class Dialog -->
          <v-dialog v-model="scDialogOpen" max-width="640" persistent>
            <v-card>
              <v-card-title>
                {{ scDialogEditing ? "Edit Storage Class" : "Create Storage Class" }}
              </v-card-title>
              <v-card-text>
                <v-text-field
                  v-model="scForm.name"
                  label="Name"
                  variant="outlined"
                  density="comfortable"
                  :disabled="scDialogEditing"
                  :rules="[v => !!v || 'Name is required']"
                  class="mb-2"
                />
                <v-text-field
                  v-model="scForm.provisioner"
                  label="Provisioner"
                  placeholder="ebs.csi.aws.com"
                  variant="outlined"
                  density="comfortable"
                  :rules="[v => !!v || 'Provisioner is required']"
                  class="mb-2"
                />
                <v-row>
                  <v-col cols="12" md="6">
                    <v-select
                      v-model="scForm.reclaim_policy"
                      :items="['Delete', 'Retain', 'Recycle']"
                      label="Reclaim Policy"
                      variant="outlined"
                      density="comfortable"
                    />
                  </v-col>
                  <v-col cols="12" md="6">
                    <v-select
                      v-model="scForm.volume_binding_mode"
                      :items="['Immediate', 'WaitForFirstConsumer']"
                      label="Volume Binding Mode"
                      variant="outlined"
                      density="comfortable"
                    />
                  </v-col>
                </v-row>
                <v-checkbox
                  v-model="scForm.allow_volume_expansion"
                  label="Allow Volume Expansion"
                  density="comfortable"
                  hide-details
                  class="mb-2"
                />
                <v-checkbox
                  v-model="scForm.is_default"
                  label="Set as Default"
                  density="comfortable"
                  hide-details
                  class="mb-4"
                />
                <v-text-field
                  v-model="scMountOptionsText"
                  label="Mount Options"
                  placeholder="debug, discard"
                  variant="outlined"
                  density="comfortable"
                  hint="Comma-separated mount options"
                  persistent-hint
                  class="mb-4"
                />
                <div class="d-flex align-center mb-2">
                  <span class="text-subtitle-2">Parameters</span>
                  <v-spacer />
                  <v-btn
                    size="x-small"
                    variant="tonal"
                    color="primary"
                    prepend-icon="mdi-plus"
                    @click="addScParamRow"
                  >
                    Add
                  </v-btn>
                </div>
                <v-row
                  v-for="(row, idx) in scParamRows"
                  :key="`sc-param-${idx}`"
                  dense
                  align="center"
                >
                  <v-col cols="5">
                    <v-text-field
                      v-model="row.key"
                      label="Key"
                      variant="outlined"
                      density="compact"
                      hide-details
                    />
                  </v-col>
                  <v-col cols="5">
                    <v-text-field
                      v-model="row.value"
                      label="Value"
                      variant="outlined"
                      density="compact"
                      hide-details
                    />
                  </v-col>
                  <v-col cols="2" class="text-right">
                    <v-btn
                      icon="mdi-delete"
                      variant="text"
                      color="error"
                      size="x-small"
                      @click="removeScParamRow(idx)"
                    />
                  </v-col>
                </v-row>
              </v-card-text>
              <v-card-actions>
                <v-spacer />
                <v-btn variant="text" @click="scDialogOpen = false">Cancel</v-btn>
                <v-btn
                  color="primary"
                  variant="flat"
                  :loading="scDialogSaving"
                  :disabled="!scForm.name || !scForm.provisioner"
                  @click="saveStorageClass"
                >
                  {{ scDialogEditing ? "Update" : "Create" }}
                </v-btn>
              </v-card-actions>
            </v-card>
          </v-dialog>

          <!-- Delete Storage Class Confirmation -->
          <ConfirmDialog
            v-model="scDeleteDialogOpen"
            title="Delete Storage Class"
            :message="`Are you sure you want to delete storage class '${scDeleteTarget}'? This cannot be undone.`"
            confirm-text="Delete"
            :loading="scDeleting"
            @confirm="deleteStorageClass"
          />
        </div>

        <!-- Networking -->
        <div v-else-if="section === 'networking'">
          <div class="d-flex align-center mb-2">
            <h3 class="text-h6">Ingress classes</h3>
            <v-spacer />
            <v-btn
              size="small"
              color="primary"
              variant="tonal"
              prepend-icon="mdi-plus"
              @click="openCreateIngressClass"
            >
              Create Ingress Class
            </v-btn>
          </div>
          <p class="text-body-2 text-secondary mb-3">
            Manage IngressClass resources in the cluster. Each class maps to a
            specific ingress controller (e.g. ALB, nginx).
          </p>

          <div v-if="ingressClasses.length === 0" class="text-center py-6 text-secondary">
            No ingress classes found in the cluster.
          </div>

          <v-table v-else density="comfortable">
            <thead>
              <tr>
                <th>Name</th>
                <th>Controller</th>
                <th>Default</th>
                <th>Parameters</th>
                <th class="text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="ic in ingressClasses" :key="ic.name">
                <td>{{ ic.name }}</td>
                <td>{{ ic.controller }}</td>
                <td>
                  <v-chip
                    v-if="ic.is_default"
                    size="x-small"
                    color="primary"
                    variant="tonal"
                  >
                    default
                  </v-chip>
                </td>
                <td>
                  <span v-if="ic.parameters" class="text-caption">
                    {{ ic.parameters.kind }}/{{ ic.parameters.name }}
                  </span>
                  <span v-else class="text-secondary">-</span>
                </td>
                <td class="text-right">
                  <v-btn
                    icon="mdi-pencil"
                    variant="text"
                    size="small"
                    @click="openEditIngressClass(ic)"
                  />
                  <v-btn
                    icon="mdi-delete"
                    variant="text"
                    color="error"
                    size="small"
                    @click="confirmDeleteIngressClass(ic.name)"
                  />
                </td>
              </tr>
            </tbody>
          </v-table>

          <v-dialog v-model="icDialogOpen" max-width="640" persistent>
            <v-card>
              <v-card-title>
                {{ icDialogEditing ? "Edit Ingress Class" : "Create Ingress Class" }}
              </v-card-title>
              <v-card-text>
                <v-text-field
                  v-model="icForm.name"
                  label="Name"
                  variant="outlined"
                  density="comfortable"
                  :disabled="icDialogEditing"
                  :rules="[v => !!v || 'Name is required']"
                  class="mb-2"
                />
                <v-text-field
                  v-model="icForm.controller"
                  label="Controller"
                  placeholder="ingress.k8s.aws/alb"
                  variant="outlined"
                  density="comfortable"
                  :rules="[v => !!v || 'Controller is required']"
                  class="mb-2"
                />
                <v-checkbox
                  v-model="icForm.is_default"
                  label="Set as Default"
                  density="comfortable"
                  hide-details
                  class="mb-4"
                />
                <v-checkbox
                  v-model="icShowParameters"
                  label="Configure parameters reference"
                  density="comfortable"
                  hide-details
                  class="mb-4"
                />
                <template v-if="icShowParameters">
                  <v-text-field
                    :model-value="icForm.parameters?.api_group ?? ''"
                    label="API Group"
                    placeholder="elbv2.k8s.aws"
                    variant="outlined"
                    density="comfortable"
                    class="mb-2"
                    @update:model-value="(v: string) => {
                      if (!icForm.parameters) icForm.parameters = { kind: '', name: '' };
                      icForm.parameters.api_group = v || null;
                    }"
                  />
                  <v-text-field
                    :model-value="icForm.parameters?.kind ?? ''"
                    label="Kind"
                    placeholder="IngressClassParams"
                    variant="outlined"
                    density="comfortable"
                    :rules="[v => !!v || 'Kind is required']"
                    class="mb-2"
                    @update:model-value="(v: string) => {
                      if (!icForm.parameters) icForm.parameters = { kind: '', name: '' };
                      icForm.parameters.kind = v;
                    }"
                  />
                  <v-text-field
                    :model-value="icForm.parameters?.name ?? ''"
                    label="Name"
                    placeholder="my-params"
                    variant="outlined"
                    density="comfortable"
                    :rules="[v => !!v || 'Name is required']"
                    class="mb-2"
                    @update:model-value="(v: string) => {
                      if (!icForm.parameters) icForm.parameters = { kind: '', name: '' };
                      icForm.parameters.name = v;
                    }"
                  />
                  <v-row>
                    <v-col cols="12" md="6">
                      <v-text-field
                        :model-value="icForm.parameters?.namespace ?? ''"
                        label="Namespace"
                        placeholder="kube-system"
                        variant="outlined"
                        density="comfortable"
                        @update:model-value="(v: string) => {
                          if (!icForm.parameters) icForm.parameters = { kind: '', name: '' };
                          icForm.parameters.namespace = v || null;
                        }"
                      />
                    </v-col>
                    <v-col cols="12" md="6">
                      <v-select
                        :model-value="icForm.parameters?.scope ?? 'Cluster'"
                        :items="['Cluster', 'Namespace']"
                        label="Scope"
                        variant="outlined"
                        density="comfortable"
                        @update:model-value="(v: string) => {
                          if (!icForm.parameters) icForm.parameters = { kind: '', name: '' };
                          icForm.parameters.scope = v;
                        }"
                      />
                    </v-col>
                  </v-row>
                </template>
              </v-card-text>
              <v-card-actions>
                <v-spacer />
                <v-btn variant="text" @click="icDialogOpen = false">Cancel</v-btn>
                <v-btn
                  color="primary"
                  variant="flat"
                  :loading="icDialogSaving"
                  :disabled="!icForm.name || !icForm.controller"
                  @click="saveIngressClass"
                >
                  {{ icDialogEditing ? "Update" : "Create" }}
                </v-btn>
              </v-card-actions>
            </v-card>
          </v-dialog>

          <ConfirmDialog
            v-model="icDeleteDialogOpen"
            title="Delete Ingress Class"
            :message="`Are you sure you want to delete ingress class '${icDeleteTarget}'? This cannot be undone.`"
            confirm-text="Delete"
            :loading="icDeleting"
            @confirm="deleteIngressClass"
          />
        </div>

        <!-- Authentication -->
        <div v-else-if="section === 'auth'">
          <v-alert
            v-if="auth.enabled"
            type="warning"
            variant="tonal"
            class="mb-4"
            prepend-icon="mdi-shield-lock"
          >
            <strong>Authentication is enforced.</strong> All API requests must
            carry a valid Microsoft Entra bearer token. Users without one will
            be redirected to sign in on the next request.
          </v-alert>
          <v-alert
            v-else
            type="warning"
            variant="tonal"
            class="mb-4"
            prepend-icon="mdi-alert-circle-outline"
          >
            <strong>Enabling auth will require all users to sign in via
            Microsoft Entra.</strong> Confirm the Tenant ID and Client ID
            below match a working App Registration before you flip the
            toggle -- an invalid config will lock everyone out. The backend
            reads this setting once at pod start, so a change here requires
            restarting the deckwatch pods to take effect.
          </v-alert>

          <v-switch
            v-model="auth.enabled"
            color="primary"
            label="Enable Entra authentication"
            :disabled="!auth.tenant_id || !auth.client_id"
            hint="Requires Tenant ID and Client ID to be set."
            persistent-hint
            class="mb-4"
          />

          <v-text-field
            v-model="auth.tenant_id"
            label="Tenant ID"
            placeholder="00000000-0000-0000-0000-000000000000"
            variant="outlined"
            density="comfortable"
            class="mb-2"
          />

          <v-text-field
            v-model="auth.client_id"
            label="Client (application) ID"
            placeholder="00000000-0000-0000-0000-000000000000"
            variant="outlined"
            density="comfortable"
            class="mb-2"
          />

          <v-text-field
            v-model="auth.redirect_uri"
            label="Redirect URI (optional)"
            placeholder="https://deckwatch.example.com/auth/callback"
            variant="outlined"
            density="comfortable"
            hint="Defaults to current origin + /auth/callback"
            persistent-hint
            class="mb-2"
          />

          <v-text-field
            v-model="auth.scopes"
            label="Scopes"
            placeholder="openid profile email"
            variant="outlined"
            density="comfortable"
          />
        </div>

        <!-- AI Providers -->
        <div v-else-if="section === 'ai_providers'">
          <h3 class="text-h6 mb-2">AI Integrations</h3>
          <p class="text-body-2 text-secondary mb-4">
            Controls which AI agents show up in the "Diagnose with AI" and
            "Fix with AI" flows. Turning Claude off hides the Diagnose button
            entirely across every pod view. These toggles apply to all users.
          </p>

          <v-card variant="outlined" class="mb-3 pa-4">
            <div class="d-flex align-center">
              <v-icon
                icon="mdi-alpha-c-circle"
                color="deep-purple"
                size="large"
                class="mr-3"
              />
              <div class="flex-grow-1">
                <div class="text-subtitle-1">Claude</div>
                <div class="text-caption text-secondary">
                  Anthropic Claude AI for diagnostics and code fixes.
                </div>
              </div>
              <v-switch
                v-model="aiClaudeEnabled"
                color="primary"
                hide-details
                density="compact"
                inset
              />
            </div>

            <template v-if="aiClaudeEnabled">
              <v-divider class="my-4" />

              <v-select
                :model-value="aiProvider.type"
                :items="AI_PROVIDER_OPTIONS"
                item-title="title"
                item-value="value"
                label="API Provider"
                variant="outlined"
                density="comfortable"
                class="mb-4"
                @update:model-value="(v: AiProviderType) => {
                  if (v === 'native') {
                    aiProvider = { type: 'native' };
                  } else if (v === 'vertex_ai') {
                    aiProvider = { type: 'vertex_ai', project_id: '', region: 'us-central1' };
                  } else {
                    aiProvider = { type: 'bedrock', region: 'us-east-1', model_id: 'anthropic.claude-sonnet-4-20250514-v1:0' };
                  }
                }"
              />

          <!-- Native: Anthropic API key -->
          <template v-if="aiProvider.type === 'native'">
            <v-card variant="outlined" class="mb-3 pa-4">
              <div class="d-flex align-center mb-2">
                <v-icon icon="mdi-key-variant" color="deep-purple" class="mr-2" />
                <span class="text-subtitle-2">Anthropic API Key</span>
                <v-spacer />
                <v-chip
                  v-if="credentialStatus.anthropic_api_key"
                  size="small" color="success" variant="tonal"
                >Configured</v-chip>
                <v-chip v-else size="small" color="warning" variant="tonal">Not set</v-chip>
              </div>
              <v-text-field
                v-model="anthropicKeyInput"
                label="API key"
                placeholder="sk-ant-api03-..."
                variant="outlined"
                density="comfortable"
                type="password"
                hint="Paste a new key to replace the current one. Leave blank to keep existing."
                persistent-hint
                class="mb-2"
              />
              <div class="d-flex">
                <v-btn
                  v-if="credentialStatus.anthropic_api_key"
                  variant="text" color="error" size="small" prepend-icon="mdi-delete"
                  :loading="savingCredentials"
                  @click="clearCredential('anthropic_api_key')"
                >Remove</v-btn>
              </div>
            </v-card>
          </template>

          <!-- Vertex AI: project, region, GCP SA key -->
          <template v-if="aiProvider.type === 'vertex_ai'">
            <v-row class="mb-2">
              <v-col cols="12" md="6">
                <v-text-field
                  v-model="aiProvider.project_id"
                  label="GCP Project ID"
                  placeholder="my-gcp-project"
                  variant="outlined"
                  density="comfortable"
                />
              </v-col>
              <v-col cols="12" md="6">
                <v-text-field
                  v-model="aiProvider.region"
                  label="Region"
                  placeholder="us-east5"
                  variant="outlined"
                  density="comfortable"
                />
              </v-col>
            </v-row>
            <v-card variant="outlined" class="mb-3 pa-4">
              <div class="d-flex align-center mb-2">
                <v-icon icon="mdi-key-variant" color="blue" class="mr-2" />
                <span class="text-subtitle-2">GCP Service Account Key</span>
                <v-spacer />
                <v-chip
                  v-if="credentialStatus.gcp_sa_key"
                  size="small" color="success" variant="tonal"
                >Configured</v-chip>
                <v-chip v-else size="small" color="warning" variant="tonal">Not set</v-chip>
              </div>
              <v-textarea
                v-model="gcpSaKeyInput"
                label="Service account JSON key"
                placeholder='{"type": "service_account", ...}'
                variant="outlined"
                density="comfortable"
                rows="3"
                auto-grow
                hint="Paste the full JSON key file contents."
                persistent-hint
                class="mb-2"
              />
              <div class="d-flex">
                <v-btn
                  v-if="credentialStatus.gcp_sa_key"
                  variant="text" color="error" size="small" prepend-icon="mdi-delete"
                  :loading="savingCredentials"
                  @click="clearCredential('gcp_sa_key')"
                >Remove</v-btn>
              </div>
            </v-card>
          </template>

          <!-- Bedrock: region, model -->
          <template v-if="aiProvider.type === 'bedrock'">
            <v-alert type="info" variant="tonal" class="mb-4">
              AWS Bedrock support is coming soon. SigV4 request signing is
              required and not yet implemented.
            </v-alert>
            <v-row>
              <v-col cols="12" md="6">
                <v-text-field
                  v-model="aiProvider.region"
                  label="AWS Region"
                  placeholder="us-east-1"
                  variant="outlined"
                  density="comfortable"
                />
              </v-col>
              <v-col cols="12" md="6">
                <v-text-field
                  v-model="aiProvider.model_id"
                  label="Model ID"
                  placeholder="anthropic.claude-sonnet-4-20250514-v1:0"
                  variant="outlined"
                  density="comfortable"
                />
              </v-col>
            </v-row>
          </template>

          <v-btn
            v-if="aiProvider.type !== 'bedrock'"
            color="primary"
            variant="tonal"
            prepend-icon="mdi-lock"
            :loading="savingCredentials"
            :disabled="!anthropicKeyInput && !gcpSaKeyInput"
            @click="saveCredentials"
            class="mt-2"
          >
            Save Credentials
          </v-btn>

            </template>
          </v-card>
        </div>

        <!-- Observability -->
        <div v-else-if="section === 'observability'">
          <h3 class="text-h6 mb-2">Prometheus monitoring</h3>
          <p class="text-body-2 text-secondary mb-3">
            When enabled, deckwatch can create PodMonitor resources for
            per-deployment metrics scraping. Requires the prometheus-operator
            CRDs (monitoring.coreos.com) to be installed in the cluster.
          </p>
          <v-switch
            v-model="prometheusEnabled"
            color="primary"
            label="Enable Prometheus monitoring"
            hide-details
            inset
            density="compact"
          />

          <v-divider class="my-6" />

          <h3 class="text-h6 mb-2">Cluster alert notifications</h3>
          <p class="text-body-2 text-secondary mb-3">
            When enabled, deckwatch pops a toast in the top-right of every
            page for each new cluster Warning event. Toasts auto-dismiss
            after 5 seconds. This setting is stored in this browser only.
          </p>
          <v-switch
            v-model="alertsEnabled"
            color="primary"
            label="Enable cluster alert notifications"
            hide-details
            inset
            density="compact"
          />

          <v-divider class="my-6" />

          <h3 class="text-h6 mb-2">Agent Feedback</h3>
          <p class="text-body-2 text-secondary mb-3">
            When enabled, an MCP tool (<code>submit_agent_feedback</code>) is
            exposed to connected agents so they can record observations about
            missing tooling, suboptimal workflows, or situations where better
            guidance would have helped. Feedback is reviewed in the
            <strong>Agent Feedback</strong> settings section. Defaults to
            disabled.
          </p>
          <v-switch
            v-model="agentFeedbackEnabled"
            color="primary"
            label="Enable Agent Feedback"
            hide-details
            inset
            density="compact"
          />
        </div>

        <!-- Agent Feedback -->
        <div v-else-if="section === 'agent_feedback'">
          <AgentFeedbackPage />
        </div>

        <!-- Templates -->
        <div v-else-if="section === 'templates'">
          <div class="d-flex align-center mb-2">
            <h3 class="text-h6">Deployment templates</h3>
            <v-spacer />
            <v-btn
              size="small"
              color="primary"
              variant="tonal"
              prepend-icon="mdi-plus"
              @click="addTemplate"
            >
              Add custom template
            </v-btn>
          </div>
          <p class="text-body-2 text-secondary mb-4">
            Templates pre-fill the "Create Deployment" form. Edits to a
            builtin entry are stored as an override in the
            <code>deckwatch-templates</code> ConfigMap -- the compiled-in
            default is untouched, so "Reset to Default" always restores it.
            Custom entries (id not shared with a builtin) are persisted
            wholesale.
          </p>

          <div v-if="loading" class="d-flex justify-center pa-8">
            <v-progress-circular indeterminate color="primary" />
          </div>

          <div
            v-else-if="templates.length === 0"
            class="text-center py-6 text-secondary"
          >
            No templates configured. Click "Add custom template" to create one.
          </div>

          <v-expansion-panels v-else variant="accordion" class="mb-2">
            <v-expansion-panel
              v-for="(tpl, idx) in templates"
              :key="`tpl-${tpl.id}`"
            >
              <v-expansion-panel-title>
                <div class="d-flex align-center" style="width: 100%">
                  <v-icon :icon="tpl.icon || 'mdi-cube-outline'" class="mr-3" />
                  <div class="flex-grow-1">
                    <div class="text-subtitle-1">{{ tpl.name || tpl.id }}</div>
                    <div class="text-caption text-secondary">
                      {{ tpl.id }}
                    </div>
                  </div>
                  <v-chip
                    v-if="tpl.builtin"
                    size="x-small"
                    color="primary"
                    variant="tonal"
                    class="mr-2"
                  >
                    builtin
                  </v-chip>
                  <v-chip
                    v-else
                    size="x-small"
                    color="secondary"
                    variant="tonal"
                    class="mr-2"
                  >
                    custom
                  </v-chip>
                </div>
              </v-expansion-panel-title>
              <v-expansion-panel-text>
                <v-row dense>
                  <v-col cols="12" md="4">
                    <v-text-field
                      v-model="tpl.id"
                      label="Template id"
                      density="comfortable"
                      :disabled="tpl.builtin"
                      :hint="tpl.builtin ? 'Builtin ids are fixed' : 'Lowercase, dash-separated'"
                      persistent-hint
                    />
                  </v-col>
                  <v-col cols="12" md="5">
                    <v-text-field
                      v-model="tpl.name"
                      label="Display name"
                      density="comfortable"
                    />
                  </v-col>
                  <v-col cols="12" md="3">
                    <v-select
                      v-model="tpl.category"
                      :items="TEMPLATE_CATEGORIES"
                      item-title="title"
                      item-value="value"
                      label="Category"
                      density="comfortable"
                    />
                  </v-col>

                  <v-col cols="12" md="4">
                    <v-text-field
                      v-model="tpl.icon"
                      label="MDI icon"
                      placeholder="mdi-web"
                      density="comfortable"
                    />
                  </v-col>
                  <v-col cols="12" md="8">
                    <v-textarea
                      v-model="tpl.description"
                      label="Description"
                      rows="2"
                      auto-grow
                      density="comfortable"
                    />
                  </v-col>
                </v-row>

                <v-divider class="my-4" />
                <h4 class="text-subtitle-2 mb-2">Payload defaults</h4>
                <p class="text-caption text-secondary mb-3">
                  Fields pre-filled into the "Create Deployment" form. Leave
                  <code>image</code> blank to force the operator to pick one.
                </p>
                <v-row dense>
                  <v-col cols="12" md="6">
                    <v-text-field
                      :model-value="payloadField(idx, 'image')"
                      label="Container image"
                      placeholder="nginx:1.27-alpine"
                      density="comfortable"
                      @update:model-value="setPayloadField(idx, 'image', $event)"
                    />
                  </v-col>
                  <v-col cols="12" md="3">
                    <v-text-field
                      :model-value="payloadField(idx, 'port')"
                      label="Container port"
                      type="number"
                      placeholder="80"
                      density="comfortable"
                      @update:model-value="setPayloadField(idx, 'port', $event ? Number($event) : undefined)"
                    />
                  </v-col>
                  <v-col cols="12" md="3">
                    <v-text-field
                      :model-value="payloadField(idx, 'replicas')"
                      label="Replicas"
                      type="number"
                      placeholder="1"
                      density="comfortable"
                      @update:model-value="setPayloadField(idx, 'replicas', $event ? Number($event) : undefined)"
                    />
                  </v-col>
                </v-row>

                <v-divider class="my-4" />
                <div class="d-flex align-center mb-2">
                  <h4 class="text-subtitle-2">Full payload (advanced)</h4>
                  <v-spacer />
                  <span class="text-caption text-secondary">
                    Probes, cmd/args, env, resource defaults -- anything the
                    Create API accepts.
                  </span>
                </div>
                <v-textarea
                  :model-value="stringifyPayload(tpl.payload)"
                  label="JSON payload"
                  variant="outlined"
                  density="comfortable"
                  rows="10"
                  class="font-monospace"
                  @change="updatePayloadFromString(idx, $event.target.value)"
                />

                <v-divider class="my-4" />
                <div class="d-flex">
                  <v-btn
                    v-if="tpl.builtin"
                    variant="tonal"
                    color="warning"
                    size="small"
                    prepend-icon="mdi-restore"
                    :disabled="!defaultTemplates.has(tpl.id)"
                    @click="resetTemplate(idx)"
                  >
                    Reset to default
                  </v-btn>
                  <v-spacer />
                  <v-btn
                    v-if="!tpl.builtin"
                    variant="text"
                    color="error"
                    size="small"
                    prepend-icon="mdi-delete"
                    @click="removeTemplate(idx)"
                  >
                    Delete template
                  </v-btn>
                </div>
              </v-expansion-panel-text>
            </v-expansion-panel>
          </v-expansion-panels>

          <v-divider class="my-6" />

          <div class="d-flex align-center mb-2">
            <h3 class="text-h6">Ingress templates</h3>
            <v-spacer />
            <v-btn
              size="small"
              color="primary"
              variant="tonal"
              prepend-icon="mdi-plus"
              @click="addIngressTemplate"
            >
              Add template
            </v-btn>
          </div>
          <p class="text-body-2 text-secondary mb-4">
            Named annotation presets for ingress creation. Define templates
            once (e.g. ALB annotations for EKS) and users pick one in the
            ingress dialog. The default template is applied automatically
            when no template is explicitly selected.
          </p>

          <div
            v-if="ingressTemplates.length === 0"
            class="text-center py-6 text-secondary"
          >
            No ingress templates configured. Click "Add template" to create one.
          </div>

          <v-expansion-panels v-else variant="accordion" class="mb-2">
            <v-expansion-panel
              v-for="(tpl, idx) in ingressTemplates"
              :key="`itpl-${idx}`"
            >
              <v-expansion-panel-title>
                <div class="d-flex align-center" style="width: 100%">
                  <v-icon icon="mdi-lan" class="mr-3" />
                  <div class="flex-grow-1">
                    <div class="text-subtitle-1">{{ tpl.name || "(unnamed)" }}</div>
                    <div class="text-caption text-secondary">
                      {{ tpl.ingress_class || "no class" }}
                      &middot;
                      {{ Object.keys(tpl.annotations).length }} annotation(s)
                    </div>
                  </div>
                  <v-chip
                    v-if="tpl.is_default"
                    size="x-small"
                    color="primary"
                    variant="tonal"
                    class="mr-2"
                  >
                    default
                  </v-chip>
                </div>
              </v-expansion-panel-title>
              <v-expansion-panel-text>
                <v-row dense>
                  <v-col cols="12" md="5">
                    <v-text-field
                      v-model="tpl.name"
                      label="Template name"
                      placeholder="alb-internet-facing"
                      density="comfortable"
                      :rules="[v => !!v || 'Name is required']"
                    />
                  </v-col>
                  <v-col cols="12" md="5">
                    <v-text-field
                      v-model="tpl.ingress_class"
                      label="Ingress class"
                      placeholder="alb"
                      density="comfortable"
                    />
                  </v-col>
                  <v-col cols="12" md="2" class="d-flex align-center">
                    <v-checkbox
                      :model-value="tpl.is_default"
                      label="Default"
                      density="comfortable"
                      hide-details
                      @update:model-value="(v: boolean | null) => { if (v) setIngressTemplateDefault(idx); else tpl.is_default = false; }"
                    />
                  </v-col>
                </v-row>

                <v-divider class="my-4" />

                <div class="d-flex align-center mb-2">
                  <span class="text-subtitle-2">Annotations</span>
                  <v-spacer />
                  <v-btn
                    size="x-small"
                    variant="tonal"
                    color="primary"
                    prepend-icon="mdi-plus"
                    @click="addIngressTemplateAnnotation(idx)"
                  >
                    Add
                  </v-btn>
                </div>

                <div
                  v-if="Object.keys(tpl.annotations).length === 0"
                  class="text-body-2 text-secondary mb-3"
                >
                  No annotations. Click "Add" to define key-value pairs.
                </div>

                <v-row
                  v-for="(value, key) in tpl.annotations"
                  :key="`itpl-${idx}-ann-${key}`"
                  dense
                  align="center"
                >
                  <v-col cols="5">
                    <v-text-field
                      :model-value="key"
                      label="Key"
                      variant="outlined"
                      density="compact"
                      hide-details
                      placeholder="alb.ingress.kubernetes.io/scheme"
                      @update:model-value="(v: string) => updateIngressTemplateAnnotationKey(idx, key as string, v)"
                    />
                  </v-col>
                  <v-col cols="5">
                    <v-text-field
                      :model-value="value"
                      label="Value"
                      variant="outlined"
                      density="compact"
                      hide-details
                      placeholder="internet-facing"
                      @update:model-value="(v: string) => updateIngressTemplateAnnotationValue(idx, key as string, v)"
                    />
                  </v-col>
                  <v-col cols="2" class="text-right">
                    <v-btn
                      icon="mdi-delete"
                      variant="text"
                      color="error"
                      size="x-small"
                      @click="removeIngressTemplateAnnotation(idx, key as string)"
                    />
                  </v-col>
                </v-row>

                <v-divider class="my-4" />
                <div class="d-flex justify-end">
                  <v-btn
                    variant="text"
                    color="error"
                    size="small"
                    prepend-icon="mdi-delete"
                    @click="removeIngressTemplate(idx)"
                  >
                    Delete template
                  </v-btn>
                </div>
              </v-expansion-panel-text>
            </v-expansion-panel>
          </v-expansion-panels>
        </div>

        <!-- Git Repositories -->
        <div v-else-if="section === 'git_repositories'">
          <div class="d-flex align-center mb-2">
            <h3 class="text-h6">Managed repositories</h3>
            <v-spacer />
            <v-btn
              size="small"
              color="primary"
              variant="tonal"
              prepend-icon="mdi-plus"
              @click="addRepository"
            >
              Add repository
            </v-btn>
          </div>
          <p class="text-body-2 text-secondary mb-4">
            Populates the repository dropdown in the GitOps dialog. Operators
            still have a "Custom" option for one-off URLs.
          </p>

          <div v-if="gitRepositories.length === 0" class="text-center py-6 text-secondary">
            No repositories configured. Click "Add repository" to create one.
          </div>

          <v-card
            v-for="(repo, idx) in gitRepositories"
            :key="`repo-${idx}`"
            variant="outlined"
            class="mb-3 pa-3"
          >
            <v-row dense align="center">
              <v-col cols="12" md="3">
                <v-text-field
                  v-model="repo.name"
                  label="Display name"
                  placeholder="acme-api"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="6">
                <v-text-field
                  v-model="repo.url"
                  label="Clone URL (HTTPS)"
                  placeholder="https://github.com/org/repo"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="2">
                <v-text-field
                  v-model="repo.default_branch"
                  label="Default branch"
                  placeholder="main"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="1" class="text-right">
                <v-btn
                  icon="mdi-delete"
                  variant="text"
                  color="error"
                  size="small"
                  @click="removeRepository(idx)"
                />
              </v-col>
            </v-row>
          </v-card>

          <v-divider class="my-6" />

          <!-- Git Tokens -->
          <div class="d-flex align-center mb-2">
            <h3 class="text-h6">Managed Git tokens</h3>
            <v-spacer />
            <v-btn
              size="small"
              color="primary"
              variant="tonal"
              prepend-icon="mdi-plus"
              @click="openAddTokenDialog"
            >
              Add token
            </v-btn>
          </div>
          <p class="text-body-2 text-secondary mb-4">
            Each token creates a Kubernetes Secret with a <code>token</code>
            data key. The same entry can be referenced by many deployments.
          </p>

          <div v-if="gitTokenSecrets.length === 0" class="text-center py-6 text-secondary">
            No tokens configured. Click "Add token" to create one.
          </div>

          <v-card
            v-for="t in gitTokenSecrets"
            :key="`tok-${t.secret_name}`"
            variant="outlined"
            class="mb-3 pa-3"
          >
            <v-row dense align="center">
              <v-col cols="12" md="4">
                <div class="text-subtitle-2">{{ t.name }}</div>
                <div class="text-caption text-secondary">{{ t.secret_name }}</div>
              </v-col>
              <v-col cols="12" md="4">
                <v-text-field
                  model-value="••••••••"
                  label="Token"
                  variant="outlined"
                  density="compact"
                  readonly
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="4" class="text-right">
                <v-btn
                  size="small"
                  variant="tonal"
                  color="primary"
                  class="mr-2"
                  @click="openUpdateTokenDialog(t)"
                >
                  Update
                </v-btn>
                <v-btn
                  icon="mdi-delete"
                  variant="text"
                  color="error"
                  size="small"
                  @click="confirmDeleteToken(t.secret_name)"
                />
              </v-col>
            </v-row>
          </v-card>

          <!-- Add/Update Token Dialog -->
          <v-dialog v-model="tokenDialogOpen" max-width="540" persistent>
            <v-card>
              <v-card-title>
                {{ tokenDialogMode === "create" ? "Add Git Token" : "Update Git Token" }}
              </v-card-title>
              <v-card-text>
                <v-text-field
                  v-model="tokenForm.name"
                  label="Display name"
                  placeholder="github-cicd"
                  variant="outlined"
                  density="comfortable"
                  :rules="[v => !!v || 'Display name is required']"
                  class="mb-2"
                  @blur="autoGenerateSecretName"
                />
                <v-text-field
                  v-model="tokenForm.secret_name"
                  label="Kubernetes Secret name"
                  placeholder="github-cicd-token"
                  variant="outlined"
                  density="comfortable"
                  :disabled="tokenDialogMode === 'update'"
                  :rules="[v => !!v || 'Secret name is required']"
                  class="mb-2"
                />
                <v-text-field
                  v-model="tokenForm.token"
                  label="Token value"
                  type="password"
                  variant="outlined"
                  density="comfortable"
                  :rules="[v => !!v || 'Token is required']"
                  :placeholder="tokenDialogMode === 'update' ? 'Enter new token value' : ''"
                />
              </v-card-text>
              <v-card-actions>
                <v-spacer />
                <v-btn variant="text" @click="tokenDialogOpen = false">Cancel</v-btn>
                <v-btn
                  color="primary"
                  variant="flat"
                  :loading="tokenDialogSaving"
                  :disabled="!tokenForm.name || !tokenForm.secret_name || !tokenForm.token"
                  @click="saveTokenSecret"
                >
                  {{ tokenDialogMode === "create" ? "Create" : "Update" }}
                </v-btn>
              </v-card-actions>
            </v-card>
          </v-dialog>

          <!-- Delete Token Confirmation -->
          <ConfirmDialog
            v-model="tokenDeleteDialogOpen"
            title="Delete Git Token"
            :message="`Are you sure you want to delete token '${tokenDeleteTarget}'? This will also delete the Kubernetes Secret.`"
            confirm-text="Delete"
            :loading="tokenDeleting"
            @confirm="deleteTokenSecret"
          />
        </div>

        <!-- Container Registries -->
        <div v-else-if="section === 'container_registries'">
          <div class="d-flex align-center mb-2">
            <h3 class="text-h6">Managed registries</h3>
            <v-spacer />
            <v-btn
              size="small"
              color="primary"
              variant="tonal"
              prepend-icon="mdi-plus"
              @click="addRegistry"
            >
              Add registry
            </v-btn>
          </div>
          <p class="text-body-2 text-secondary mb-4">
            Any OCI-compliant registry is accepted. Kaniko pushes builds to
            <code>{url}:{tag}</code>, so include the repository path when the
            registry requires it (e.g. <code>docker.io/myorg/api</code>).
          </p>

          <div v-if="ociRegistries.length === 0" class="text-center py-6 text-secondary">
            No registries configured. Click "Add registry" to create one.
          </div>

          <v-card
            v-for="(reg, idx) in ociRegistries"
            :key="`reg-${idx}`"
            variant="outlined"
            class="mb-3 pa-3"
          >
            <v-row dense align="center">
              <v-col cols="12" md="3">
                <v-text-field
                  v-model="reg.name"
                  label="Display name"
                  placeholder="acme-ecr"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="5">
                <v-text-field
                  v-model="reg.url"
                  label="Registry URL"
                  placeholder="591839118651.dkr.ecr.us-gov-west-1.amazonaws.com/apps/my-app"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="3">
                <v-select
                  v-model="reg.registry_type"
                  :items="REGISTRY_TYPES"
                  item-title="title"
                  item-value="value"
                  label="Type"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="1" class="text-right">
                <v-btn
                  icon="mdi-delete"
                  variant="text"
                  color="error"
                  size="small"
                  @click="removeRegistry(idx)"
                />
              </v-col>
            </v-row>
          </v-card>
        </div>

        <!-- Plugins -->
        <div v-else-if="section === 'plugins'">

          <!-- Marketplace browser (collapsible) -->
          <v-expansion-panels v-model="marketplaceOpen" class="mb-6">
            <v-expansion-panel value="marketplace">
              <v-expansion-panel-title>
                <v-icon class="mr-2" color="primary">mdi-store</v-icon>
                <span class="font-weight-medium">Browse Marketplace</span>
                <span class="text-caption text-medium-emphasis ml-2">
                  Discover and install plugins from the deckwatch catalog
                </span>
              </v-expansion-panel-title>
              <v-expansion-panel-text class="pa-0">
                <MarketplacePage />
              </v-expansion-panel-text>
            </v-expansion-panel>
          </v-expansion-panels>

          <div class="d-flex align-center justify-space-between mb-4">
            <div>
              <div class="text-h6">Installed Plugins</div>
              <div class="text-caption text-medium-emphasis">
                External WASM plugins fetched from Git repositories. Each plugin runs on every
                deployment create/update and can inject env vars, sidecars, and Kubernetes
                resources for annotated deployments.
              </div>
            </div>
            <v-btn
              prepend-icon="mdi-plus"
              variant="tonal"
              color="primary"
              size="small"
              @click="addPlugin"
            >
              Add Plugin
            </v-btn>
          </div>

          <!-- Loaded plugins overview -->
          <h3 class="text-subtitle-1 mb-2">Loaded plugins</h3>
          <p class="text-body-2 text-secondary mb-3">
            Plugins currently loaded in deckwatch. "Configure" is shown when a plugin declares
            configuration fields.
          </p>

          <v-alert
            v-if="loadedPluginsError"
            type="warning"
            density="compact"
            variant="tonal"
            class="mb-3"
          >
            {{ loadedPluginsError }}
          </v-alert>

          <div
            v-else-if="loadedPlugins.length === 0"
            class="text-center py-4 text-secondary text-body-2 mb-4"
          >
            No plugins are currently loaded.
          </div>

          <v-table v-else density="comfortable" class="mb-6">
            <thead>
              <tr>
                <th>Name</th>
                <th>Version</th>
                <th>Description</th>
                <th>Status</th>
                <th class="text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="lp in loadedPlugins" :key="lp.name">
                <td>
                  <div class="d-flex align-center ga-2">
                    <v-icon icon="mdi-puzzle" size="small" color="primary" />
                    <span class="font-weight-medium">{{ lp.name }}</span>
                  </div>
                </td>
                <td>
                  <span class="text-caption text-secondary">{{ lp.version || "—" }}</span>
                </td>
                <td>
                  <span class="text-body-2">{{ lp.description || "—" }}</span>
                </td>
                <td>
                  <v-chip
                    :color="
                      plugins.find((p) => p.name === lp.name)?.enabled !== false
                        ? 'success'
                        : 'warning'
                    "
                    size="x-small"
                    variant="tonal"
                  >
                    {{
                      plugins.find((p) => p.name === lp.name)?.enabled !== false
                        ? "loaded"
                        : "disabled"
                    }}
                  </v-chip>
                </td>
                <td class="text-right">
                  <v-btn
                    v-if="lp.config_schema.length > 0"
                    size="small"
                    variant="tonal"
                    color="primary"
                    prepend-icon="mdi-cog"
                    @click="router.push({ name: 'plugin-settings', params: { name: lp.name } })"
                  >
                    Configure
                  </v-btn>
                  <span v-else class="text-caption text-secondary">No config fields</span>
                </td>
              </tr>
            </tbody>
          </v-table>

          <v-divider class="mb-6" />

          <div class="text-subtitle-1 mb-3">Plugin sources</div>

          <v-card
            v-for="(plugin, idx) in plugins"
            :key="idx"
            variant="outlined"
            class="mb-4 pa-4"
          >
            <div class="d-flex align-center justify-space-between mb-3">
              <div class="d-flex align-center ga-2">
                <v-icon :icon="plugin.enabled ? 'mdi-puzzle' : 'mdi-puzzle-outline'" size="small" :color="plugin.enabled ? 'primary' : 'disabled'" />
                <span class="text-subtitle-2">{{ plugin.name || 'Unnamed plugin' }}</span>
              </div>
              <div class="d-flex align-center ga-2">
                <v-switch
                  v-model="plugin.enabled"
                  density="compact"
                  hide-details
                  color="primary"
                  :label="plugin.enabled ? 'Enabled' : 'Disabled'"
                />
                <v-btn
                  icon="mdi-delete"
                  variant="text"
                  color="error"
                  size="x-small"
                  @click="removePlugin(idx)"
                />
              </div>
            </div>

            <v-row dense>
              <v-col cols="12" sm="6">
                <v-text-field
                  v-model="plugin.name"
                  label="Name"
                  hint="Unique identifier used in annotation keys (lowercase alphanumeric + hyphens)"
                  density="compact"
                  variant="outlined"
                  persistent-hint
                />
              </v-col>
              <v-col cols="12" sm="6">
                <v-select
                  :model-value="plugin.source.type"
                  :items="[{ title: 'GitHub', value: 'github' }, { title: 'HTTPS URL', value: 'url' }]"
                  label="Source type"
                  density="compact"
                  variant="outlined"
                  @update:model-value="(v: string) => {
                    if (v === 'github') plugin.source = { type: 'github', repo: '', ref: 'main', path: 'plugin.wasm', use_release: false };
                    else plugin.source = { type: 'url', url: '' };
                  }"
                />
              </v-col>
            </v-row>

            <!-- GitHub source -->
            <template v-if="plugin.source.type === 'github'">
              <v-row dense class="mt-1">
                <v-col cols="12" sm="6">
                  <v-text-field
                    v-model="(plugin.source as any).repo"
                    label="Repository"
                    placeholder="owner/repo"
                    density="compact"
                    variant="outlined"
                  />
                </v-col>
                <v-col cols="6" sm="3">
                  <v-text-field
                    v-model="(plugin.source as any).ref"
                    label="Ref"
                    placeholder="v1.0.0"
                    density="compact"
                    variant="outlined"
                  />
                </v-col>
                <v-col cols="6" sm="3">
                  <v-text-field
                    v-model="(plugin.source as any).path"
                    label="Path"
                    placeholder="plugin.wasm"
                    density="compact"
                    variant="outlined"
                  />
                </v-col>
              </v-row>
              <v-row dense>
                <v-col cols="12" sm="6">
                  <v-switch
                    v-model="(plugin.source as any).use_release"
                    label="Fetch from GitHub Releases (recommended for tagged versions)"
                    density="compact"
                    hide-details
                    color="primary"
                  />
                </v-col>
                <v-col cols="12" sm="6">
                  <v-text-field
                    v-model="plugin.token_secret"
                    label="Token secret (optional)"
                    hint="Name of a git_token_secrets entry for private repos"
                    density="compact"
                    variant="outlined"
                    clearable
                    persistent-hint
                  />
                </v-col>
              </v-row>
            </template>

            <!-- URL source -->
            <template v-else>
              <v-row dense class="mt-1">
                <v-col cols="12">
                  <v-text-field
                    v-model="(plugin.source as any).url"
                    label="URL"
                    placeholder="https://artifacts.example.com/plugin.wasm"
                    density="compact"
                    variant="outlined"
                  />
                </v-col>
              </v-row>
              <v-row dense>
                <v-col cols="12" sm="6">
                  <v-text-field
                    v-model="plugin.token_secret"
                    label="Token secret (optional)"
                    density="compact"
                    variant="outlined"
                    clearable
                  />
                </v-col>
              </v-row>
            </template>

            <!-- Allowed hosts (shared by both source types) -->
            <v-divider class="my-3" />
            <div class="text-caption text-medium-emphasis mb-2">
              <strong>Network access</strong> — hosts the plugin can reach via HTTP.
              Supports globs (e.g. <code>*.amazonaws.com</code>, <code>vault.corp.internal</code>).
            </div>
            <v-combobox
              v-model="plugin.allowed_hosts"
              label="Allowed hosts"
              density="compact"
              variant="outlined"
              multiple
              chips
              closable-chips
              hide-details
              class="mb-3"
              placeholder="Add a host and press Enter"
            />

            <!-- Inherit env keys -->
            <div class="text-caption text-medium-emphasis mb-2">
              <strong>Inherit env vars</strong> — environment variable names to read from the
              deckwatch pod at invocation time and pass to the plugin. Use this for credentials
              already mounted as pod env vars (e.g. via a Kubernetes Secret) so they don't
              need to be stored in settings. These override same-named keys in the config map above.
            </div>
            <v-combobox
              v-model="plugin.inherit_env_keys"
              label="Inherit env var names"
              density="compact"
              variant="outlined"
              multiple
              chips
              closable-chips
              hide-details
              class="mb-3"
              placeholder="e.g. AWS_ACCESS_KEY_ID — add and press Enter"
            />

            <!-- Inherit env file keys -->
            <div class="text-caption text-medium-emphasis mb-2 mt-3">
              <strong>Inherit file contents</strong> — reads the contents of files whose paths
              are in env vars, injecting them as plugin config keys. Used for workload identity
              tokens (e.g. IRSA). The plugin handles what to do with the content.
            </div>
            <v-row
              v-for="(envVar, configKey) in plugin.inherit_env_file_keys"
              :key="configKey"
              dense
              class="mb-1"
            >
              <v-col cols="5">
                <v-text-field
                  :model-value="configKey"
                  label="Config key"
                  placeholder="AWS_IDENTITY_TOKEN"
                  density="compact"
                  variant="outlined"
                  hide-details
                  @update:model-value="(newKey: string) => {
                    const val = plugin.inherit_env_file_keys[configKey];
                    delete plugin.inherit_env_file_keys[configKey];
                    plugin.inherit_env_file_keys[newKey] = val;
                  }"
                />
              </v-col>
              <v-col cols="6">
                <v-text-field
                  v-model="plugin.inherit_env_file_keys[configKey]"
                  label="Env var holding file path"
                  placeholder="AWS_WEB_IDENTITY_TOKEN_FILE"
                  density="compact"
                  variant="outlined"
                  hide-details
                />
              </v-col>
              <v-col cols="1" class="d-flex align-center justify-center">
                <v-btn icon="mdi-close" variant="text" size="x-small"
                  @click="delete plugin.inherit_env_file_keys[configKey]" />
              </v-col>
            </v-row>
            <v-btn
              variant="text"
              size="small"
              prepend-icon="mdi-plus"
              class="mb-3"
              @click="plugin.inherit_env_file_keys[''] = ''"
            >
              Add file key
            </v-btn>

            <!-- Config key-value pairs -->
            <div class="text-caption text-medium-emphasis mb-2">
              <strong>Plugin config</strong> — injected as key-value pairs the plugin reads via
              <code>extism_pdk::config::get()</code>. Use for credentials, endpoints, or any
              plugin-specific settings (e.g. <code>AWS_ACCESS_KEY_ID</code>, <code>VAULT_TOKEN</code>).
            </div>
            <v-row
              v-for="(_, key) in plugin.config"
              :key="key"
              dense
              class="mb-1"
            >
              <v-col cols="5">
                <v-text-field
                  :model-value="key"
                  label="Key"
                  density="compact"
                  variant="outlined"
                  hide-details
                  @update:model-value="(newKey: string) => {
                    const val = plugin.config[key];
                    delete plugin.config[key];
                    plugin.config[newKey] = val;
                  }"
                />
              </v-col>
              <v-col cols="6">
                <v-text-field
                  v-model="plugin.config[key]"
                  label="Value"
                  density="compact"
                  variant="outlined"
                  hide-details
                  :type="key.toLowerCase().includes('secret') || key.toLowerCase().includes('password') || key.toLowerCase().includes('token') ? 'password' : 'text'"
                />
              </v-col>
              <v-col cols="1" class="d-flex align-center justify-center">
                <v-btn icon="mdi-close" variant="text" size="x-small" @click="delete plugin.config[key]" />
              </v-col>
            </v-row>
            <v-btn
              variant="text"
              size="small"
              prepend-icon="mdi-plus"
              @click="plugin.config[''] = ''"
            >
              Add config entry
            </v-btn>
          </v-card>

          <div v-if="plugins.length === 0" class="text-center py-8 text-secondary text-body-2">
            No plugins configured. Click "Add Plugin" to register an external WASM plugin.
          </div>
        </div>

        <!-- MCP Tuning -->
        <div v-else-if="section === 'mcp_tuning'">
          <div class="mb-4">
            <div class="text-h6">MCP Tuning</div>
            <div class="text-caption text-medium-emphasis">
              Org-specific guidance injected into MCP tool descriptions. Each hint is appended
              only to tools in that resource group — the AI sees namespace guidance when working
              with namespaces, deployment guidance when working with deployments, and so on.
              Keeps instructions contextual rather than flooding every interaction.
            </div>
          </div>

          <v-card variant="outlined" class="mb-4">
            <v-card-title class="text-subtitle-2 d-flex align-center ga-2">
              <v-icon icon="mdi-earth" size="small" />
              Global instructions
            </v-card-title>
            <v-card-text>
              <div class="text-caption text-medium-emphasis mb-2">
                Included in every MCP session via the <code>initialize</code> response.
                Use sparingly — applies to all tool interactions regardless of resource type.
              </div>
              <v-textarea
                v-model="mcpTuning.global"
                placeholder="e.g. All infrastructure changes must be approved by the platform team before deployment."
                density="compact"
                variant="outlined"
                rows="3"
                auto-grow
                hide-details
              />
            </v-card-text>
          </v-card>

          <v-row dense>
            <v-col
              v-for="group in mcpTuningGroups"
              :key="group.key"
              cols="12"
              md="6"
            >
              <v-card variant="outlined" class="pa-3 fill-height">
                <div class="d-flex align-center ga-2 mb-1">
                  <v-icon :icon="group.icon" size="small" color="primary" />
                  <span class="text-subtitle-2">{{ group.label }}</span>
                </div>
                <div class="d-flex flex-wrap ga-1 mb-2">
                  <v-chip
                    v-for="example in group.examples"
                    :key="example"
                    size="x-small"
                    variant="tonal"
                    color="secondary"
                  >{{ example }}</v-chip>
                </div>
                <v-textarea
                  v-model="(mcpTuning as any)[group.key]"
                  :placeholder="group.placeholder"
                  density="compact"
                  variant="outlined"
                  rows="2"
                  auto-grow
                  hide-details
                />
              </v-card>
            </v-col>
          </v-row>

          <!-- Per-plugin MCP tuning subsections -->
          <template
            v-for="lp in loadedPlugins.filter((p: PluginSummary) => p.mcp_tuning_fields && p.mcp_tuning_fields.length > 0)"
            :key="lp.name"
          >
            <v-divider class="my-4" />
            <div class="mb-3">
              <div class="d-flex align-center ga-2 mb-1">
                <v-icon icon="mdi-puzzle-outline" size="small" color="primary" />
                <span class="text-subtitle-1 font-weight-medium">Plugin: {{ lp.name }}</span>
                <v-chip size="x-small" variant="tonal" color="primary">{{ lp.version }}</v-chip>
              </div>
              <div class="text-caption text-medium-emphasis mb-3">
                Plugin-declared naming conventions injected into MCP sessions when enabled.
              </div>
              <v-row dense>
                <v-col
                  v-for="field in (lp.mcp_tuning_fields as McpTuningField[])"
                  :key="field.key"
                  cols="12"
                  md="6"
                >
                  <v-text-field
                    :model-value="plugins.find((p: PluginConfig) => p.name === lp.name)?.mcp_tuning?.[field.key] ?? ''"
                    :label="field.label"
                    :placeholder="field.placeholder || field.default || ''"
                    :hint="field.description"
                    persistent-hint
                    variant="outlined"
                    density="comfortable"
                    @update:model-value="(v: string) => setPluginMcpTuning(lp.name, field.key, v)"
                  />
                </v-col>
              </v-row>
            </div>
          </template>
        </div>

        <!-- Advanced -->
        <div v-else-if="section === 'advanced'">
          <div class="text-h6 font-weight-bold mb-4">Advanced</div>
          <v-row>
            <v-col cols="12" md="8">
              <v-text-field
                v-model="marketplaceUrl"
                label="Marketplace URL"
                :placeholder="'http://market.deckwatch.io/catalog.json'"
                variant="outlined"
                density="compact"
                hint="URL of the plugin marketplace catalog JSON. Leave blank to disable. Point to an internal mirror for air-gapped environments."
                persistent-hint
                clearable
              />
            </v-col>
          </v-row>
          <v-row class="mt-4">
            <v-col>
              <v-btn color="primary" @click="saveAdvanced">Save</v-btn>
            </v-col>
          </v-row>
        </div>

        <!-- Audit Log -->
        <div v-else-if="section === 'audit'">
          <AuditLogPage />
        </div>
      </v-card>
    </div>

    <v-snackbar v-model="snackbar" :color="snackbarColor" location="top">
      {{ snackbarMessage }}
      <template #actions>
        <v-btn variant="text" @click="snackbar = false">Close</v-btn>
      </template>
    </v-snackbar>
  </div>
</template>
