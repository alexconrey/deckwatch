<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { settingsApi } from "@/api/settings";
import { gitopsApi } from "@/api/gitops";
import { secretsApi } from "@/api/secrets";
import type {
  DeckwatchSettings,
  GitOpsConfig,
  GitOpsConfigRequest,
  GitRepository,
  GitTokenSecret,
  OciRegistry,
} from "@/types/api";
import RegistryTagPicker from "@/components/common/RegistryTagPicker.vue";

const CUSTOM_SENTINEL = "__custom__";

const props = defineProps<{
  modelValue: boolean;
  initialConfig?: GitOpsConfig | null;
  loading?: boolean;
  /** Namespace of the deployment being edited. Used as a fallback when the
   *  selected token entry doesn't specify one. */
  namespace?: string;
  /** Deployment name. Appended as the repo path when the user selects the
   *  built-in deckwatch registry, whose URL is a bare hostname (kaniko's
   *  --destination requires host/repo, not just host). */
  deploymentName?: string;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  save: [config: GitOpsConfigRequest];
}>();

// Settings-backed dropdown sources.
const settingsLoading = ref(false);
const settingsError = ref<string | null>(null);
const gitRepositories = ref<GitRepository[]>([]);
const ociRegistries = ref<OciRegistry[]>([]);
const gitTokenSecrets = ref<GitTokenSecret[]>([]);

// Selected keys (repo name / token name / registry name, or CUSTOM_SENTINEL).
const selectedRepoName = ref<string>("");
const selectedTokenName = ref<string>("");
const selectedRegistryName = ref<string>("");

// Form state — bound to the underlying values sent to the API. Populated
// either by the dropdown selection or by the "Custom" text field.
const form = ref({
  repoUrl: "",
  branch: "main",
  tokenSecretName: "",
  dockerfilePath: "Dockerfile",
  dockerContext: ".",
  ociRepository: "",
  includePaths: "",
  excludePaths: "",
  pollInterval: 60,
  webhookEnabled: false,
  // Webhook signing secret. Sent to the API only when the user types
  // something new — an empty value is treated as "don't touch" so an
  // unrelated Save doesn't wipe an already-configured secret. See
  // `handleSave` for the encoding.
  webhookSecret: "",
});

/** Whether the backend already has a webhook secret stored for this
 *  deployment. Sourced from the read-only `webhook_secret_configured`
 *  field on the get_config response; used to render the "Configured" hint
 *  and to change the input placeholder from "set a secret" to "rotate". */
const webhookSecretConfigured = ref(false);

const showWebhookSecret = ref(false);
const webhookSecretCopied = ref(false);
const webhookUrlCopied = ref(false);

const registryPickerOpen = ref(false);

// Create-new-token inline flow state.
const showCreateToken = ref(false);
const newTokenSecretName = ref("");
const newTokenValue = ref("");
const createTokenLoading = ref(false);
const createTokenError = ref<string | null>(null);

async function handleCreateToken() {
  const ns = props.namespace;
  if (!ns) {
    createTokenError.value = "Namespace is required to create a secret.";
    return;
  }
  const secretName = newTokenSecretName.value.trim();
  if (!secretName) {
    createTokenError.value = "Secret name is required.";
    return;
  }
  if (!newTokenValue.value) {
    createTokenError.value = "Token value is required.";
    return;
  }
  createTokenLoading.value = true;
  createTokenError.value = null;
  try {
    await secretsApi.create(ns, {
      name: secretName,
      data: { token: newTokenValue.value },
    });
    // Auto-fill the token secret field with the new secret name.
    form.value.tokenSecretName = secretName;
    selectedTokenName.value = CUSTOM_SENTINEL;
    // Reset and close the inline form.
    showCreateToken.value = false;
    newTokenSecretName.value = "";
    newTokenValue.value = "";
  } catch (e) {
    createTokenError.value =
      e instanceof Error ? e.message : "Failed to create secret";
  } finally {
    createTokenLoading.value = false;
  }
}

// Branch autocomplete state.
const branchOptions = ref<string[]>([]);
const branchLoading = ref(false);
const branchError = ref<string | null>(null);

const repoItems = computed(() => [
  ...gitRepositories.value.map((r) => ({ title: r.name, subtitle: r.url, value: r.name })),
  { title: "Custom URL…", subtitle: "Type your own", value: CUSTOM_SENTINEL },
]);
const tokenItems = computed(() => [
  ...gitTokenSecrets.value.map((t) => ({
    title: t.name,
    subtitle: `${t.namespace}/${t.secret_name}`,
    value: t.name,
  })),
  { title: "Custom Secret name…", subtitle: "Type your own", value: CUSTOM_SENTINEL },
]);
const registryItems = computed(() => [
  ...ociRegistries.value.map((r) => ({
    title: r.name,
    subtitle: `${r.registry_type} · ${r.url}`,
    value: r.name,
  })),
  { title: "Custom URL…", subtitle: "Type your own", value: CUSTOM_SENTINEL },
]);

const useCustomRepo = computed(() => selectedRepoName.value === CUSTOM_SENTINEL);
const useCustomToken = computed(() => selectedTokenName.value === CUSTOM_SENTINEL);
const useCustomRegistry = computed(() => selectedRegistryName.value === CUSTOM_SENTINEL);

// The embedded deckwatch registry is the only source the picker can browse,
// so only show the Browse button when that entry is present in settings.
const canBrowseRegistry = computed(() =>
  ociRegistries.value.some(
    (r) => r.builtin && r.registry_type === "deckwatch",
  ),
);

/** Detect the Git host and format a hint about which provider will handle
 *  incoming webhook deliveries. Pure UI — the server does its own detection
 *  when the delivery arrives, so a mismatch here can't produce a wrong
 *  build. Just tells the user what to configure on the provider side. */
const detectedProvider = computed(() => {
  const url = form.value.repoUrl.toLowerCase();
  try {
    const u = new URL(url.startsWith("http") ? url : `https://${url}`);
    const host = u.hostname;
    if (host === "github.com" || host.endsWith(".github.com")) return "GitHub";
    if (host === "gitlab.com" || host.startsWith("gitlab.")) return "GitLab";
    if (host === "bitbucket.org" || host.startsWith("bitbucket.")) return "Bitbucket";
    return null;
  } catch {
    return null;
  }
});

/** URL to hand to the Git provider's webhook configuration UI. Uses the
 *  currently-open page's origin so operators don't have to know the
 *  deckwatch external hostname — copy from the same URL bar they came
 *  from. When deckwatch runs behind a reverse proxy the origin matches
 *  the public URL. */
const webhookUrl = computed(() => {
  if (typeof window === "undefined") return "";
  return `${window.location.origin}/api/webhooks/gitops`;
});

/** Copy the incoming initialConfig into the local form. Called only when the
 *  dialog transitions from closed → open (or on first mount if it starts
 *  open) — NOT on every prop change. The parent's `status` object is
 *  refreshed by a 5s poll, so `initialConfig` gets a new reference
 *  continuously; if we hydrated on every change we would stomp the user's
 *  in-progress edits. */
function hydrateFormFromInitialConfig() {
  const v = props.initialConfig;
  if (!v) return;
  form.value = {
    repoUrl: v.repo_url,
    branch: v.branch,
    tokenSecretName: v.token_secret,
    dockerfilePath: v.dockerfile_path,
    dockerContext: v.docker_context,
    // Server sends both; prefer oci_repository and fall back for old bundles.
    ociRepository: v.oci_repository || v.ecr_repository || "",
    includePaths: v.include_paths.join(", "),
    excludePaths: v.exclude_paths.join(", "),
    pollInterval: v.poll_interval_seconds,
    webhookEnabled: v.webhook_enabled,
    webhookSecret: "",
  };
  // Older API bundles don't send this — coerce to false rather than
  // undefined so the "Configured" hint stays honest.
  webhookSecretConfigured.value = Boolean(v.webhook_secret_configured);
  reconcileSelectionsFromForm();
}

async function loadSettings() {
  settingsLoading.value = true;
  settingsError.value = null;
  try {
    const s: DeckwatchSettings = await settingsApi.get();
    gitRepositories.value = s.git_repositories ?? [];
    ociRegistries.value = s.oci_registries ?? [];
    gitTokenSecrets.value = s.git_token_secrets ?? [];
    reconcileSelectionsFromForm();
  } catch (e) {
    settingsError.value =
      e instanceof Error ? e.message : "Failed to load settings";
  } finally {
    settingsLoading.value = false;
  }
}

/** The built-in deckwatch registry stores just a hostname (e.g.
 *  `deckwatch-registry.deckwatch.svc.cluster.local:5000`) since it hosts many
 *  images. Kaniko's --destination requires `host/repo:tag`, so when the user
 *  picks that entry we append the deployment name as the repo path. External
 *  registry entries (ghcr.io/org/repo, ECR ARNs, etc.) are assumed to already
 *  contain a repo path and are used as-is. */
function resolveRegistryUrl(reg: OciRegistry): string {
  if (reg.builtin && reg.registry_type === "deckwatch" && props.deploymentName) {
    return `${reg.url}/${props.deploymentName}`;
  }
  return reg.url;
}

/** After settings load, match the currently-populated form values against
 *  the managed lists to pre-select the right dropdown entries. Falls back
 *  to the Custom sentinel when the value isn't in the list. */
function reconcileSelectionsFromForm() {
  const repoMatch = gitRepositories.value.find((r) => r.url === form.value.repoUrl);
  selectedRepoName.value = repoMatch
    ? repoMatch.name
    : form.value.repoUrl
    ? CUSTOM_SENTINEL
    : "";

  const tokenMatch = gitTokenSecrets.value.find(
    (t) => t.secret_name === form.value.tokenSecretName,
  );
  selectedTokenName.value = tokenMatch
    ? tokenMatch.name
    : form.value.tokenSecretName
    ? CUSTOM_SENTINEL
    : "";

  const regMatch = ociRegistries.value.find(
    (r) => r.url === form.value.ociRepository || resolveRegistryUrl(r) === form.value.ociRepository,
  );
  selectedRegistryName.value = regMatch
    ? regMatch.name
    : form.value.ociRepository
    ? CUSTOM_SENTINEL
    : "";

  if (regMatch && regMatch.builtin && regMatch.registry_type === "deckwatch") {
    const upgraded = resolveRegistryUrl(regMatch);
    if (form.value.ociRepository === regMatch.url && upgraded !== regMatch.url) {
      form.value.ociRepository = upgraded;
    }
  }
}

watch(
  () => props.modelValue,
  (open) => {
    if (open) {
      hydrateFormFromInitialConfig();
      loadSettings();
    }
  },
);

// When the user picks a managed repo, populate the URL + default branch.
watch(selectedRepoName, (name) => {
  if (!name || name === CUSTOM_SENTINEL) return;
  const repo = gitRepositories.value.find((r) => r.name === name);
  if (!repo) return;
  form.value.repoUrl = repo.url;
  if (repo.default_branch && !form.value.branch) {
    form.value.branch = repo.default_branch;
  }
});

// When the user picks a managed token, populate the underlying secret name.
watch(selectedTokenName, (name) => {
  if (!name || name === CUSTOM_SENTINEL) return;
  const tok = gitTokenSecrets.value.find((t) => t.name === name);
  if (!tok) return;
  form.value.tokenSecretName = tok.secret_name;
});

// When the user picks a managed registry, populate the URL.
watch(selectedRegistryName, (name) => {
  if (!name || name === CUSTOM_SENTINEL) return;
  const reg = ociRegistries.value.find((r) => r.name === name);
  if (!reg) return;
  form.value.ociRepository = resolveRegistryUrl(reg);
});

// Registry picker returns `host/repo:tag`. For the GitOps repository field
// we want the pushable repository (`host/repo`) since GitOps generates the
// tag per build — strip the tag off if the user picked one.
function onRegistryPicked(imageRef: string) {
  const colon = imageRef.lastIndexOf(":");
  const slash = imageRef.lastIndexOf("/");
  const repoOnly = colon > slash ? imageRef.slice(0, colon) : imageRef;
  form.value.ociRepository = repoOnly;
  selectedRegistryName.value = CUSTOM_SENTINEL;
}

/** Fires the branch listing API when the user opens/types in the branch
 *  autocomplete. Requires a resolved repo URL + selected managed token. */
async function fetchBranches() {
  branchError.value = null;
  if (!form.value.repoUrl) return;
  if (
    !selectedTokenName.value ||
    selectedTokenName.value === CUSTOM_SENTINEL
  ) {
    branchError.value =
      "Select a managed Git token to enable live branch discovery.";
    return;
  }
  branchLoading.value = true;
  try {
    const res = await gitopsApi.listBranches({
      repoUrl: form.value.repoUrl,
      tokenSecret: selectedTokenName.value,
      namespace: props.namespace,
    });
    branchOptions.value = res.branches;
    if (!form.value.branch && res.default_branch) {
      form.value.branch = res.default_branch;
    }
  } catch (e) {
    branchError.value =
      e instanceof Error ? e.message : "Failed to list branches";
  } finally {
    branchLoading.value = false;
  }
}

/** Generate a URL-safe random secret. 32 bytes of entropy encoded as
 *  base64url — matches what GitHub's own "generate" button produces. Uses
 *  crypto.getRandomValues so it's cryptographically strong. */
function generateWebhookSecret() {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  const b64 = btoa(String.fromCharCode(...bytes));
  form.value.webhookSecret = b64
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
  showWebhookSecret.value = true;
}

async function copyToClipboard(text: string, kind: "url" | "secret") {
  try {
    await navigator.clipboard.writeText(text);
    if (kind === "url") {
      webhookUrlCopied.value = true;
      setTimeout(() => (webhookUrlCopied.value = false), 1500);
    } else {
      webhookSecretCopied.value = true;
      setTimeout(() => (webhookSecretCopied.value = false), 1500);
    }
  } catch {
    // Older Safari / iframes without clipboard-write permission — no
    // graceful fallback here since the user can still highlight + copy
    // from the text field.
  }
}

const canSave = computed(
  () =>
    !!form.value.repoUrl &&
    !!form.value.ociRepository,
);

const handleSave = () => {
  const config: GitOpsConfigRequest = {
    repo_url: form.value.repoUrl,
    branch: form.value.branch,
    token_secret: form.value.tokenSecretName,
    dockerfile_path: form.value.dockerfilePath,
    docker_context: form.value.dockerContext,
    oci_repository: form.value.ociRepository,
    poll_interval_seconds: form.value.pollInterval,
    webhook_enabled: form.value.webhookEnabled,
  };

  // Only send the webhook secret when the user actually typed one, so a
  // Save with the field left blank doesn't overwrite an existing secret
  // on the server. Rotation = type a new value; delete = disable gitops.
  if (form.value.webhookSecret) {
    config.webhook_secret = form.value.webhookSecret;
  }

  if (form.value.includePaths.trim()) {
    config.include_paths = form.value.includePaths
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
  }
  if (form.value.excludePaths.trim()) {
    config.exclude_paths = form.value.excludePaths
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
  }

  emit("save", config);
};

onMounted(() => {
  if (props.modelValue) {
    hydrateFormFromInitialConfig();
    loadSettings();
  }
});
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="720"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card>
      <v-card-title>
        {{ initialConfig ? "Edit GitOps" : "Enable GitOps" }}
      </v-card-title>
      <v-card-text>
        <v-alert
          v-if="settingsError"
          type="warning"
          density="compact"
          variant="tonal"
          class="mb-3"
        >
          {{ settingsError }}
        </v-alert>

        <!-- Repository -->
        <v-select
          v-model="selectedRepoName"
          :items="repoItems"
          item-title="title"
          item-value="value"
          label="Repository"
          variant="outlined"
          density="comfortable"
          :loading="settingsLoading"
          hint="Managed in Settings → Git Repositories"
          persistent-hint
          class="mb-1"
        >
          <template #item="{ props: itemProps, item }">
            <v-list-item v-bind="itemProps" :title="item.raw.title" :subtitle="item.raw.subtitle" />
          </template>
        </v-select>
        <v-text-field
          v-if="useCustomRepo"
          v-model="form.repoUrl"
          label="Custom repository URL"
          placeholder="https://github.com/org/repo"
          variant="outlined"
          density="comfortable"
          class="mb-2"
        />

        <v-row dense>
          <v-col cols="6">
            <v-autocomplete
              v-model="form.branch"
              :items="branchOptions"
              label="Branch"
              placeholder="main"
              variant="outlined"
              density="comfortable"
              :loading="branchLoading"
              :error-messages="branchError ? [branchError] : []"
              :menu-props="{ maxHeight: 300 }"
              auto-select-first
              @focus="fetchBranches"
              @update:search="() => {
                if (!branchOptions.length && !branchLoading) fetchBranches();
              }"
            />
          </v-col>
          <v-col cols="6">
            <v-select
              v-model="selectedTokenName"
              :items="tokenItems"
              item-title="title"
              item-value="value"
              label="Git Token"
              variant="outlined"
              density="comfortable"
              :loading="settingsLoading"
              hint="Managed in Settings → Git Tokens"
              persistent-hint
            >
              <template #item="{ props: itemProps, item }">
                <v-list-item v-bind="itemProps" :title="item.raw.title" :subtitle="item.raw.subtitle" />
              </template>
            </v-select>
          </v-col>
        </v-row>
        <v-text-field
          v-if="useCustomToken"
          v-model="form.tokenSecretName"
          label="Custom Kubernetes Secret name"
          placeholder="my-git-token"
          variant="outlined"
          density="comfortable"
          class="mb-2"
        >
          <template #append-inner>
            <v-tooltip location="top" text="Create a new K8s secret with a Git token">
              <template #activator="{ props: tipProps }">
                <v-btn
                  v-bind="tipProps"
                  size="small"
                  variant="tonal"
                  density="comfortable"
                  @click="showCreateToken = !showCreateToken"
                >
                  Create New
                </v-btn>
              </template>
            </v-tooltip>
          </template>
        </v-text-field>

        <!-- Inline create-token form -->
        <v-expand-transition>
          <v-sheet
            v-if="showCreateToken && useCustomToken"
            class="pa-3 mb-3 rounded"
            color="grey-lighten-4"
            border
          >
            <div class="text-subtitle-2 mb-2">Create Git Token Secret</div>
            <v-alert
              v-if="createTokenError"
              type="error"
              density="compact"
              variant="tonal"
              class="mb-2"
              closable
              @click:close="createTokenError = null"
            >
              {{ createTokenError }}
            </v-alert>
            <v-text-field
              v-model="newTokenSecretName"
              label="Secret Name"
              placeholder="my-git-token"
              variant="outlined"
              density="compact"
              hide-details
              class="mb-2"
            />
            <v-text-field
              v-model="newTokenValue"
              label="Token Value"
              type="password"
              placeholder="ghp_... or glpat-..."
              variant="outlined"
              density="compact"
              hide-details
              class="mb-2"
            />
            <div class="d-flex justify-end ga-2">
              <v-btn
                size="small"
                variant="text"
                @click="showCreateToken = false"
              >
                Cancel
              </v-btn>
              <v-btn
                size="small"
                variant="flat"
                color="primary"
                :loading="createTokenLoading"
                :disabled="!newTokenSecretName.trim() || !newTokenValue"
                @click="handleCreateToken"
              >
                Save
              </v-btn>
            </div>
          </v-sheet>
        </v-expand-transition>

        <!-- OCI Registry -->
        <v-select
          v-model="selectedRegistryName"
          :items="registryItems"
          item-title="title"
          item-value="value"
          label="OCI Registry"
          variant="outlined"
          density="comfortable"
          :loading="settingsLoading"
          hint="Managed in Settings → OCI Registries. Any OCI-compliant registry works."
          persistent-hint
          class="mb-1"
        >
          <template #item="{ props: itemProps, item }">
            <v-list-item v-bind="itemProps" :title="item.raw.title" :subtitle="item.raw.subtitle" />
          </template>
        </v-select>
        <v-row v-if="useCustomRegistry" dense align="center" class="mb-2">
          <v-col :cols="canBrowseRegistry ? 10 : 12">
            <v-text-field
              v-model="form.ociRepository"
              label="Custom registry URL"
              placeholder="ghcr.io/myorg/api or docker.io/myorg/api"
              variant="outlined"
              density="comfortable"
              hide-details
            />
          </v-col>
          <v-col v-if="canBrowseRegistry" cols="2">
            <v-tooltip location="top" text="Browse the deckwatch registry">
              <template #activator="{ props: tipProps }">
                <v-btn
                  v-bind="tipProps"
                  icon="mdi-image-search"
                  variant="tonal"
                  density="comfortable"
                  @click="registryPickerOpen = true"
                />
              </template>
            </v-tooltip>
          </v-col>
        </v-row>

        <v-row dense>
          <v-col cols="6">
            <v-text-field
              v-model="form.dockerfilePath"
              label="Dockerfile Path"
              placeholder="Dockerfile"
              density="comfortable"
            />
          </v-col>
          <v-col cols="6">
            <v-text-field
              v-model="form.dockerContext"
              label="Build Context"
              placeholder="."
              density="comfortable"
            />
          </v-col>
        </v-row>

        <v-divider class="my-3" />
        <div class="text-subtitle-2 mb-2">Path Filters</div>

        <v-text-field
          v-model="form.includePaths"
          label="Include Paths (comma-separated)"
          placeholder="src/, Dockerfile"
          density="compact"
          class="mb-1"
        />
        <v-text-field
          v-model="form.excludePaths"
          label="Exclude Paths (comma-separated)"
          placeholder="docs/, README.md"
          density="compact"
        />

        <v-divider class="my-3" />
        <div class="text-subtitle-2 mb-2">Polling</div>

        <v-row dense>
          <v-col cols="6">
            <v-text-field
              v-model.number="form.pollInterval"
              label="Poll Interval (seconds)"
              type="number"
              density="compact"
            />
          </v-col>
          <v-col cols="6" class="d-flex align-center">
            <v-switch
              v-model="form.webhookEnabled"
              label="Enable Webhook"
              color="primary"
              hide-details
              density="compact"
            />
          </v-col>
        </v-row>

        <!-- Webhook configuration. Only shown when the switch is on so an
             operator using polling-only mode isn't distracted by URLs and
             secrets they don't need. -->
        <template v-if="form.webhookEnabled">
          <v-divider class="my-3" />
          <div class="text-subtitle-2 mb-2 d-flex align-center">
            Webhook
            <v-chip
              v-if="detectedProvider"
              size="x-small"
              class="ml-2"
              color="primary"
              variant="tonal"
            >
              {{ detectedProvider }}
            </v-chip>
          </div>

          <div class="text-caption mb-2">
            Configure the URL below in your Git provider's webhook settings.
            {{
              detectedProvider === "GitHub"
                ? "Content type: application/json. Event: push. Sign with the secret below."
                : detectedProvider === "GitLab"
                ? "Trigger: Push events. Paste the secret below into the Secret Token field."
                : detectedProvider === "Bitbucket"
                ? "Trigger: Repository push. Paste the secret into the Bitbucket webhook Secret field."
                : "Set the endpoint to accept push events and sign with the secret below."
            }}
          </div>

          <v-text-field
            :model-value="webhookUrl"
            label="Webhook URL"
            variant="outlined"
            density="comfortable"
            readonly
            hide-details
            class="mb-2"
          >
            <template #append-inner>
              <v-tooltip
                location="top"
                :text="webhookUrlCopied ? 'Copied!' : 'Copy URL'"
              >
                <template #activator="{ props: tipProps }">
                  <v-btn
                    v-bind="tipProps"
                    :icon="webhookUrlCopied ? 'mdi-check' : 'mdi-content-copy'"
                    size="small"
                    variant="text"
                    density="comfortable"
                    @click="copyToClipboard(webhookUrl, 'url')"
                  />
                </template>
              </v-tooltip>
            </template>
          </v-text-field>

          <v-text-field
            v-model="form.webhookSecret"
            :label="
              webhookSecretConfigured
                ? 'Rotate webhook secret (leave blank to keep current)'
                : 'Webhook signing secret'
            "
            :type="showWebhookSecret ? 'text' : 'password'"
            :placeholder="
              webhookSecretConfigured
                ? '••••••••••••••••••••'
                : 'Paste from provider or click Generate'
            "
            :hint="
              webhookSecretConfigured
                ? 'Configured. A new value here will replace the existing secret.'
                : 'Required — the receiver rejects any delivery without a matching signature.'
            "
            persistent-hint
            variant="outlined"
            density="comfortable"
          >
            <template #append-inner>
              <v-tooltip
                location="top"
                :text="showWebhookSecret ? 'Hide' : 'Show'"
              >
                <template #activator="{ props: tipProps }">
                  <v-btn
                    v-bind="tipProps"
                    :icon="showWebhookSecret ? 'mdi-eye-off' : 'mdi-eye'"
                    size="small"
                    variant="text"
                    density="comfortable"
                    @click="showWebhookSecret = !showWebhookSecret"
                  />
                </template>
              </v-tooltip>
              <v-tooltip
                location="top"
                :text="webhookSecretCopied ? 'Copied!' : 'Copy'"
              >
                <template #activator="{ props: tipProps }">
                  <v-btn
                    v-bind="tipProps"
                    :icon="webhookSecretCopied ? 'mdi-check' : 'mdi-content-copy'"
                    size="small"
                    variant="text"
                    density="comfortable"
                    :disabled="!form.webhookSecret"
                    @click="copyToClipboard(form.webhookSecret, 'secret')"
                  />
                </template>
              </v-tooltip>
            </template>
          </v-text-field>

          <div class="d-flex mt-2">
            <v-btn
              size="small"
              variant="tonal"
              prepend-icon="mdi-key-plus"
              @click="generateWebhookSecret"
            >
              Generate secret
            </v-btn>
            <v-spacer />
            <v-chip
              v-if="webhookSecretConfigured"
              size="small"
              variant="tonal"
              color="success"
              prepend-icon="mdi-shield-check"
            >
              Configured
            </v-chip>
          </div>
        </template>
      </v-card-text>
      <v-card-actions>
        <v-spacer />
        <v-btn variant="text" @click="emit('update:modelValue', false)">
          Cancel
        </v-btn>
        <v-btn
          color="primary"
          variant="flat"
          :loading="loading"
          :disabled="!canSave"
          @click="handleSave"
        >
          Save
        </v-btn>
      </v-card-actions>
    </v-card>

    <RegistryTagPicker
      v-model="registryPickerOpen"
      @selected="onRegistryPicked"
    />
  </v-dialog>
</template>
