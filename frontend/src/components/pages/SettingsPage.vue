<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { settingsApi } from "@/api/settings";
import { templatesApi } from "@/api/templates";
import { useAiSettings } from "@/composables/useAiSettings";
import { useClusterAlertSettings } from "@/composables/useClusterAlertSettings";
import AuditLogPage from "@/components/pages/AuditLogPage.vue";
import type {
  AuthSettings,
  CostSettings,
  DeckwatchSettings,
  DeploymentTemplate,
  GitRepository,
  GitTokenSecret,
  NotificationEventType,
  NotificationSettings,
  OciRegistry,
  OciRegistryType,
  ResourceDefaults,
  TemplateCategory,
} from "@/types/api";

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
  | "auth"
  | "ai_providers"
  | "observability"
  | "templates"
  | "git_repositories"
  | "container_registries"
  | "audit";

const navItems: { id: SectionId; title: string; icon: string }[] = [
  { id: "general", title: "General", icon: "mdi-tune" },
  { id: "auth", title: "Authentication", icon: "mdi-shield-account" },
  { id: "ai_providers", title: "AI Providers", icon: "mdi-robot" },
  { id: "observability", title: "Observability", icon: "mdi-chart-line" },
  { id: "templates", title: "Templates", icon: "mdi-shape-outline" },
  { id: "git_repositories", title: "Git Repositories", icon: "mdi-git" },
  { id: "container_registries", title: "Container Registries", icon: "mdi-package-variant" },
  { id: "audit", title: "Audit Log", icon: "mdi-clipboard-text-clock" },
];

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
const registryEnabled = ref(false);

// AI provider toggles are now server-side settings, persisted alongside the
// rest of DeckwatchSettings so an admin toggle applies to all users.
const aiClaudeEnabled = ref(true);
const aiCodexEnabled = ref(true);

const gitRepositories = ref<GitRepository[]>([]);
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
  ociRegistries.value = s.oci_registries ?? [];
  gitTokenSecrets.value = s.git_token_secrets ?? [];
  prometheusEnabled.value = s.prometheus_enabled ?? true;
  registryEnabled.value = s.registry_enabled ?? false;
  aiClaudeEnabled.value = s.ai_claude_enabled ?? true;
  aiCodexEnabled.value = s.ai_codex_enabled ?? true;
}

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const [s, t] = await Promise.all([
      settingsApi.get(),
      templatesApi.list(),
    ]);
    applySettings(s);
    applyTemplates(t.templates);
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to load settings";
  } finally {
    loading.value = false;
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
    registry_enabled: registryEnabled.value,
    ai_claude_enabled: aiClaudeEnabled.value,
    ai_codex_enabled: aiCodexEnabled.value,
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
    if (!t.name || !t.secret_name || !t.namespace) {
      return "Every Git token needs a name, secret name, and namespace.";
    }
    if (seen.has(`tok:${t.name}`)) return `Duplicate token name: ${t.name}`;
    seen.add(`tok:${t.name}`);
  }
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

function addTokenSecret() {
  gitTokenSecrets.value.push({ name: "", secret_name: "", namespace: "" });
}
function removeTokenSecret(idx: number) {
  gitTokenSecrets.value.splice(idx, 1);
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

            <h3 class="text-h6 mb-2">Container Registry</h3>
            <p class="text-body-2 text-secondary mb-3">
              When enabled, the Registry page is accessible from the
              navigation bar. Disable to hide the registry UI entirely.
            </p>
            <v-switch
              v-model="registryEnabled"
              color="primary"
              label="Enable container registry"
              hide-details
              inset
              density="compact"
            />

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
                  Anthropic Claude Code CLI. Runs as a Kubernetes Job with an
                  <code>ANTHROPIC_API_KEY</code> Secret mounted in.
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
          </v-card>

          <v-card variant="outlined" class="mb-3 pa-4">
            <div class="d-flex align-center">
              <v-icon
                icon="mdi-alpha-o-circle"
                color="grey"
                size="large"
                class="mr-3"
              />
              <div class="flex-grow-1">
                <div class="d-flex align-center">
                  <span class="text-subtitle-1">Codex</span>
                  <v-chip
                    size="x-small"
                    color="info"
                    variant="tonal"
                    class="ml-2"
                  >
                    Coming Soon
                  </v-chip>
                </div>
                <div class="text-caption text-secondary">
                  OpenAI Codex CLI. Backend plumbing is in place; provider
                  wiring will ship in a follow-up.
                </div>
              </div>
              <v-switch
                v-model="aiCodexEnabled"
                color="primary"
                hide-details
                density="compact"
                :disabled="true"
                inset
              />
            </div>
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
              @click="addTokenSecret"
            >
              Add token
            </v-btn>
          </div>
          <p class="text-body-2 text-secondary mb-4">
            Points at a Kubernetes Secret with a <code>token</code> data key.
            The same entry can be referenced by many deployments -- no more
            per-deployment secret typing.
          </p>

          <div v-if="gitTokenSecrets.length === 0" class="text-center py-6 text-secondary">
            No tokens configured. Click "Add token" to create one.
          </div>

          <v-card
            v-for="(t, idx) in gitTokenSecrets"
            :key="`tok-${idx}`"
            variant="outlined"
            class="mb-3 pa-3"
          >
            <v-row dense align="center">
              <v-col cols="12" md="3">
                <v-text-field
                  v-model="t.name"
                  label="Display name"
                  placeholder="github-cicd"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="4">
                <v-text-field
                  v-model="t.secret_name"
                  label="Secret name"
                  placeholder="github-cicd-token"
                  density="comfortable"
                  hide-details
                />
              </v-col>
              <v-col cols="12" md="4">
                <v-text-field
                  v-model="t.namespace"
                  label="Namespace"
                  placeholder="deckwatch"
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
                  @click="removeTokenSecret(idx)"
                />
              </v-col>
            </v-row>
          </v-card>
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
