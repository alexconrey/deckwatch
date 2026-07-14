<script setup lang="ts">
import { computed, ref, watch, onMounted } from "vue";
import { useRouter } from "vue-router";
import { useNamespaceStore } from "@/stores/namespace";
import { useDeploymentsStore } from "@/stores/deployments";
import { usePolling } from "@/composables/usePolling";
import { cronjobsApi } from "@/api/cronjobs";
import { secretsApi } from "@/api/secrets";
import { configmapsApi } from "@/api/configmaps";
import { settingsApi } from "@/api/settings";
import { ApiError } from "@/api/client";
import type {
  CostSettings,
  CronJobSummary,
  DeploymentSummary,
  SecretSummary,
  SecretDetail,
  ConfigMapSummary,
  ConfigMapDetail,
} from "@/types/api";
import StatusChip from "@/components/common/StatusChip.vue";
import ReplicaGauge from "@/components/views/deployment/ReplicaGauge.vue";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import { deploymentsApi } from "@/api/deployments";
import { formatAge, formatTimestamp } from "@/utils/format";
import { estimateCost, formatCost } from "@/utils/cost";

const router = useRouter();
const ns = useNamespaceStore();
const deployments = useDeploymentsStore();

// --- Cost overlay ---
// One-shot load: cost rates rarely change and re-fetching every 5s alongside
// deployments would swamp the settings endpoint. A stale rate is fine — the
// worst case is a chip that lags a rate change by a page refresh.
const costSettings = ref<CostSettings | null>(null);
onMounted(async () => {
  try {
    const s = await settingsApi.get();
    costSettings.value = s.cost ?? null;
  } catch {
    // Cost overlay is optional; a failure just hides the column.
  }
});

const costEnabled = computed(() => {
  const c = costSettings.value;
  if (!c) return false;
  return c.cost_per_cpu_hour !== null || c.cost_per_gb_hour !== null;
});

function costForDeployment(d: DeploymentSummary): string {
  const est = estimateCost(
    d.resource_requests ?? null,
    d.replicas.desired,
    costSettings.value,
  );
  if (!est) return "—";
  return `${formatCost(est.hourly, est.currency)}/hr`;
}

const search = ref("");

type TabKey = "deployments" | "cronjobs" | "secrets" | "configmaps";
// Initial tab honors ?tab= query so /deployments?tab=secrets (used by the
// removed /secrets route redirect) opens directly on the Secrets pane.
const initialTab = ((): TabKey => {
  const q = router.currentRoute.value.query.tab;
  if (q === "cronjobs" || q === "secrets" || q === "configmaps") return q;
  return "deployments";
})();
const tab = ref<TabKey>(initialTab);

// --- CronJobs state ---
const cronjobs = ref<CronJobSummary[]>([]);
const cronjobsLoading = ref(false);
const cronjobsError = ref<string | null>(null);

// --- Secrets state ---
const secrets = ref<SecretSummary[]>([]);
const secretsLoading = ref(false);
const secretsError = ref<string | null>(null);

const showCreateSecret = ref(false);
const showViewSecret = ref(false);
const viewingSecret = ref<SecretDetail | null>(null);
const revealSecretValues = ref(false);
const secretDialogSubmitting = ref(false);
const secretDialogError = ref<string | null>(null);
const secretName = ref("");
const secretType = ref("Opaque");
const secretEntries = ref<{ key: string; value: string; revealed: boolean }[]>(
  [{ key: "", value: "", revealed: false }],
);

// --- ConfigMaps state ---
const configmaps = ref<ConfigMapSummary[]>([]);
const configmapsLoading = ref(false);
const configmapsError = ref<string | null>(null);

const showCreateCm = ref(false);
const cmDialogSubmitting = ref(false);
const cmDialogError = ref<string | null>(null);
const cmDialogMode = ref<"create" | "edit">("create");
const cmName = ref("");
const cmEntries = ref<{ key: string; value: string }[]>([
  { key: "", value: "" },
]);

// RFC 1123 label
const namePattern = /^[a-z0-9]([-a-z0-9]{0,251}[a-z0-9])?$/;
const nameRules = [
  (v: string) => !!v || "Name is required",
  (v: string) =>
    namePattern.test(v) || "Lowercase alphanumeric or '-', start/end alphanumeric",
];

// --- Deployment per-row actions ---
const showRowRestartDialog = ref(false);
const showRowDeleteDialog = ref(false);
const showRowScaleDialog = ref(false);
const rowActionTarget = ref<string | null>(null);
const rowActionLoading = ref(false);
const rowScaleReplicas = ref(1);

const openRowRestart = (name: string) => {
  rowActionTarget.value = name;
  showRowRestartDialog.value = true;
};

const openRowScale = (name: string, currentReplicas: number) => {
  rowActionTarget.value = name;
  rowScaleReplicas.value = currentReplicas;
  showRowScaleDialog.value = true;
};

const openRowDelete = (name: string) => {
  rowActionTarget.value = name;
  showRowDeleteDialog.value = true;
};

const handleRowRestart = async () => {
  if (!ns.selected || !rowActionTarget.value) return;
  rowActionLoading.value = true;
  try {
    await deploymentsApi.restart(ns.selected, rowActionTarget.value);
    showRowRestartDialog.value = false;
    rowActionTarget.value = null;
    await deployments.fetchDeployments(ns.selected);
  } catch (e) {
    deployments.error = e instanceof Error ? e.message : "Failed to restart";
  } finally {
    rowActionLoading.value = false;
  }
};

const handleRowScale = async () => {
  if (!ns.selected || !rowActionTarget.value) return;
  rowActionLoading.value = true;
  try {
    await deploymentsApi.scale(ns.selected, rowActionTarget.value, rowScaleReplicas.value);
    showRowScaleDialog.value = false;
    rowActionTarget.value = null;
    await deployments.fetchDeployments(ns.selected);
  } catch (e) {
    deployments.error = e instanceof Error ? e.message : "Failed to scale";
  } finally {
    rowActionLoading.value = false;
  }
};

const handleRowDelete = async () => {
  if (!ns.selected || !rowActionTarget.value) return;
  rowActionLoading.value = true;
  try {
    await deploymentsApi.delete(ns.selected, rowActionTarget.value);
    showRowDeleteDialog.value = false;
    rowActionTarget.value = null;
    await deployments.fetchDeployments(ns.selected);
  } catch (e) {
    deployments.error = e instanceof Error ? e.message : "Failed to delete";
  } finally {
    rowActionLoading.value = false;
  }
};

// Cost column is inserted only when the overlay is configured — otherwise
// the column would render "—" for every row and add nothing but noise.
const deploymentHeaders = computed(() => {
  const base: Array<Record<string, unknown>> = [
    { title: "Name", key: "name" },
    { title: "Image", key: "image" },
    { title: "Status", key: "status", width: "140px" },
    { title: "Replicas", key: "replicas", width: "160px" },
  ];
  if (costEnabled.value) {
    base.push({ title: "Cost", key: "cost", width: "140px", sortable: false });
  }
  base.push({ title: "Age", key: "created_at", width: "140px" });
  base.push({ title: "", key: "actions", width: "64px", sortable: false });
  return base;
});

const cronjobHeaders = [
  { title: "Name", key: "name" },
  { title: "Schedule", key: "schedule", width: "160px" },
  { title: "Suspend", key: "suspend", width: "100px" },
  { title: "Active", key: "active_count", width: "100px" },
  { title: "Last Scheduled", key: "last_schedule_time", width: "180px" },
  { title: "Age", key: "created_at", width: "140px" },
];

const secretHeaders = [
  { title: "Name", key: "name" },
  { title: "Type", key: "type", width: "160px" },
  { title: "Keys", key: "keys" },
  { title: "Age", key: "created_at", width: "120px" },
  { title: "", key: "actions", width: "180px", sortable: false },
];

const cmHeaders = [
  { title: "Name", key: "name" },
  { title: "Keys", key: "keys" },
  { title: "Age", key: "created_at", width: "120px" },
  { title: "", key: "actions", width: "180px", sortable: false },
];

const fetchCronjobs = async (namespace: string) => {
  if (!namespace) return;
  cronjobsLoading.value = true;
  cronjobsError.value = null;
  try {
    const response = await cronjobsApi.list(namespace);
    cronjobs.value = response.cronjobs;
  } catch (e) {
    cronjobsError.value =
      e instanceof Error ? e.message : "Failed to fetch cronjobs";
  } finally {
    cronjobsLoading.value = false;
  }
};

const fetchSecrets = async (namespace: string) => {
  if (!namespace) return;
  secretsLoading.value = true;
  secretsError.value = null;
  try {
    const res = await secretsApi.list(namespace);
    secrets.value = res.secrets;
  } catch (e) {
    secretsError.value =
      e instanceof ApiError ? e.body.message
        : e instanceof Error ? e.message
          : "Failed to fetch secrets";
  } finally {
    secretsLoading.value = false;
  }
};

const fetchConfigMaps = async (namespace: string) => {
  if (!namespace) return;
  configmapsLoading.value = true;
  configmapsError.value = null;
  try {
    const res = await configmapsApi.list(namespace);
    configmaps.value = res.configmaps;
  } catch (e) {
    configmapsError.value =
      e instanceof ApiError ? e.body.message
        : e instanceof Error ? e.message
          : "Failed to fetch configmaps";
  } finally {
    configmapsLoading.value = false;
  }
};

const refresh = async () => {
  if (!ns.selected) return;
  await Promise.all([
    deployments.fetchDeployments(ns.selected),
    fetchCronjobs(ns.selected),
    fetchSecrets(ns.selected),
    fetchConfigMaps(ns.selected),
  ]);
};

const { refresh: manualRefresh } = usePolling(refresh, 5000);

onMounted(() => refresh());
watch(() => ns.selected, () => refresh());

const goToDetail = (name: string) => {
  router.push({
    name: "DeploymentDetail",
    params: { namespace: ns.selected, name },
  });
};

// --- Secret dialog ---

const resetSecretForm = () => {
  secretName.value = "";
  secretType.value = "Opaque";
  secretEntries.value = [{ key: "", value: "", revealed: false }];
  secretDialogError.value = null;
  secretDialogSubmitting.value = false;
};

const openCreateSecret = () => {
  resetSecretForm();
  showCreateSecret.value = true;
};

const addSecretRow = () => {
  secretEntries.value.push({ key: "", value: "", revealed: false });
};

const removeSecretRow = (i: number) => {
  secretEntries.value.splice(i, 1);
  if (secretEntries.value.length === 0) {
    secretEntries.value.push({ key: "", value: "", revealed: false });
  }
};

const submitSecret = async () => {
  if (!ns.selected) return;
  if (!namePattern.test(secretName.value)) {
    secretDialogError.value = "Invalid secret name";
    return;
  }
  const data: Record<string, string> = {};
  for (const row of secretEntries.value) {
    const k = row.key.trim();
    if (!k) continue;
    data[k] = row.value;
  }
  if (Object.keys(data).length === 0) {
    secretDialogError.value = "At least one key/value pair is required";
    return;
  }
  secretDialogSubmitting.value = true;
  secretDialogError.value = null;
  try {
    await secretsApi.create(ns.selected, {
      name: secretName.value,
      type: secretType.value || "Opaque",
      data,
    });
    showCreateSecret.value = false;
    await fetchSecrets(ns.selected);
  } catch (e) {
    secretDialogError.value =
      e instanceof ApiError ? e.body.message
        : e instanceof Error ? e.message
          : "Failed to create secret";
  } finally {
    secretDialogSubmitting.value = false;
  }
};

const openViewSecret = async (name: string) => {
  if (!ns.selected) return;
  revealSecretValues.value = false;
  showViewSecret.value = true;
  viewingSecret.value = null;
  try {
    viewingSecret.value = await secretsApi.get(ns.selected, name, false);
  } catch (e) {
    secretsError.value =
      e instanceof ApiError ? e.body.message
        : e instanceof Error ? e.message
          : "Failed to load secret";
    showViewSecret.value = false;
  }
};

const toggleRevealSecretValues = async () => {
  if (!ns.selected || !viewingSecret.value) return;
  const next = !revealSecretValues.value;
  try {
    const detail = await secretsApi.get(ns.selected, viewingSecret.value.name, next);
    viewingSecret.value = detail;
    revealSecretValues.value = next;
  } catch (e) {
    secretsError.value =
      e instanceof ApiError ? e.body.message
        : e instanceof Error ? e.message
          : "Failed to reveal secret";
  }
};

const deleteSecret = async (name: string) => {
  if (!ns.selected) return;
  if (!confirm(`Delete secret "${name}"? This cannot be undone.`)) return;
  try {
    await secretsApi.delete(ns.selected, name);
    await fetchSecrets(ns.selected);
  } catch (e) {
    secretsError.value =
      e instanceof ApiError ? e.body.message
        : e instanceof Error ? e.message
          : "Failed to delete secret";
  }
};

// --- ConfigMap dialog ---

const resetCmForm = () => {
  cmName.value = "";
  cmEntries.value = [{ key: "", value: "" }];
  cmDialogError.value = null;
  cmDialogSubmitting.value = false;
};

const openCreateCm = () => {
  cmDialogMode.value = "create";
  resetCmForm();
  showCreateCm.value = true;
};

const openEditCm = async (name: string) => {
  if (!ns.selected) return;
  cmDialogMode.value = "edit";
  resetCmForm();
  showCreateCm.value = true;
  try {
    const detail: ConfigMapDetail = await configmapsApi.get(ns.selected, name);
    cmName.value = detail.name;
    const entries = Object.entries(detail.data).map(([key, value]) => ({
      key,
      value,
    }));
    cmEntries.value =
      entries.length > 0 ? entries : [{ key: "", value: "" }];
  } catch (e) {
    cmDialogError.value =
      e instanceof ApiError ? e.body.message
        : e instanceof Error ? e.message
          : "Failed to load configmap";
  }
};

const addCmRow = () => {
  cmEntries.value.push({ key: "", value: "" });
};

const removeCmRow = (i: number) => {
  cmEntries.value.splice(i, 1);
  if (cmEntries.value.length === 0) {
    cmEntries.value.push({ key: "", value: "" });
  }
};

const submitCm = async () => {
  if (!ns.selected) return;
  if (!namePattern.test(cmName.value)) {
    cmDialogError.value = "Invalid configmap name";
    return;
  }
  const data: Record<string, string> = {};
  for (const row of cmEntries.value) {
    const k = row.key.trim();
    if (!k) continue;
    data[k] = row.value;
  }
  cmDialogSubmitting.value = true;
  cmDialogError.value = null;
  try {
    if (cmDialogMode.value === "create") {
      await configmapsApi.create(ns.selected, { name: cmName.value, data });
    } else {
      await configmapsApi.update(ns.selected, cmName.value, {
        name: cmName.value,
        data,
      });
    }
    showCreateCm.value = false;
    await fetchConfigMaps(ns.selected);
  } catch (e) {
    cmDialogError.value =
      e instanceof ApiError ? e.body.message
        : e instanceof Error ? e.message
          : "Failed to save configmap";
  } finally {
    cmDialogSubmitting.value = false;
  }
};

const deleteCm = async (name: string) => {
  if (!ns.selected) return;
  if (!confirm(`Delete configmap "${name}"? This cannot be undone.`)) return;
  try {
    await configmapsApi.delete(ns.selected, name);
    await fetchConfigMaps(ns.selected);
  } catch (e) {
    configmapsError.value =
      e instanceof ApiError ? e.body.message
        : e instanceof Error ? e.message
          : "Failed to delete configmap";
  }
};
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <h2 class="text-h5">Resources</h2>
      <v-spacer />
      <v-btn
        variant="text"
        prepend-icon="mdi-refresh"
        :loading="deployments.loading || cronjobsLoading || secretsLoading || configmapsLoading"
        class="mr-2"
        @click="manualRefresh"
      >
        Refresh
      </v-btn>
      <v-text-field
        v-if="tab === 'deployments'"
        v-model="search"
        prepend-inner-icon="mdi-magnify"
        label="Search deployments"
        density="compact"
        variant="outlined"
        hide-details
        clearable
        single-line
        style="max-width: 260px"
        class="mr-2"
      />
      <v-btn
        v-if="tab === 'deployments'"
        color="primary"
        prepend-icon="mdi-plus"
        @click="router.push({ name: 'CreateDeployment' })"
      >
        Create Deployment
      </v-btn>
      <v-btn
        v-else-if="tab === 'secrets'"
        color="primary"
        prepend-icon="mdi-plus"
        :disabled="!ns.selected"
        @click="openCreateSecret"
      >
        Create Secret
      </v-btn>
      <v-btn
        v-else-if="tab === 'configmaps'"
        color="primary"
        prepend-icon="mdi-plus"
        :disabled="!ns.selected"
        @click="openCreateCm"
      >
        Create ConfigMap
      </v-btn>
    </div>

    <v-alert v-if="!ns.selected" type="info" class="mb-4">
      Select a namespace to view resources.
    </v-alert>

    <v-tabs v-model="tab" color="primary" class="mb-4">
      <v-tab value="deployments">
        <v-icon start icon="mdi-package-variant" />
        Deployments
      </v-tab>
      <v-tab value="cronjobs">
        <v-icon start icon="mdi-clock-outline" />
        CronJobs
      </v-tab>
      <v-tab value="secrets">
        <v-icon start icon="mdi-key-variant" />
        Secrets
      </v-tab>
      <v-tab value="configmaps">
        <v-icon start icon="mdi-file-cog-outline" />
        ConfigMaps
      </v-tab>
    </v-tabs>

    <v-window v-model="tab">
      <!-- Deployments tab -->
      <v-window-item value="deployments">
        <v-alert v-if="deployments.error" type="error" class="mb-4" closable>
          {{ deployments.error }}
        </v-alert>

        <v-data-table
          v-if="ns.selected"
          :items="deployments.deployments"
          :headers="deploymentHeaders"
          :loading="deployments.loading"
          :search="search"
          item-value="name"
          hover
          class="bg-surface rounded"
          @click:row="(_: any, row: any) => goToDetail(row.item.name)"
        >
          <template v-slot:item.name="{ item }">
            <span class="text-body-1 font-weight-medium">{{ item.name }}</span>
          </template>

          <template v-slot:item.image="{ item }">
            <span class="text-body-2 text-secondary">{{ item.image }}</span>
          </template>

          <template v-slot:item.status="{ item }">
            <StatusChip :status="item.status" />
          </template>

          <template v-slot:item.replicas="{ item }">
            <ReplicaGauge :replicas="item.replicas" />
          </template>

          <template v-slot:item.cost="{ item }">
            <span class="text-body-2">{{ costForDeployment(item) }}</span>
          </template>

          <template v-slot:item.created_at="{ item }">
            <span class="text-body-2 text-secondary">
              {{ formatAge(item.created_at) }}
            </span>
          </template>

          <template v-slot:item.actions="{ item }">
            <v-menu location="bottom end">
              <template v-slot:activator="{ props: menuProps }">
                <v-btn
                  icon="mdi-dots-vertical"
                  variant="text"
                  size="small"
                  v-bind="menuProps"
                  @click.stop
                />
              </template>
              <v-list density="compact">
                <v-list-item
                  prepend-icon="mdi-restart"
                  title="Restart"
                  @click="openRowRestart(item.name)"
                />
                <v-list-item
                  prepend-icon="mdi-arrow-expand-vertical"
                  title="Scale"
                  @click="openRowScale(item.name, item.replicas.desired)"
                />
                <v-list-item
                  prepend-icon="mdi-delete"
                  title="Delete"
                  class="text-error"
                  @click="openRowDelete(item.name)"
                />
              </v-list>
            </v-menu>
          </template>

          <template v-slot:no-data>
            <div class="text-center py-8 text-secondary">
              <v-icon icon="mdi-package-variant" size="48" class="mb-2" />
              <div>No deployments in this namespace</div>
            </div>
          </template>
        </v-data-table>
      </v-window-item>

      <!-- CronJobs tab -->
      <v-window-item value="cronjobs">
        <v-alert v-if="cronjobsError" type="error" class="mb-4" closable>
          {{ cronjobsError }}
        </v-alert>

        <v-data-table
          v-if="ns.selected"
          :items="cronjobs"
          :headers="cronjobHeaders"
          :loading="cronjobsLoading"
          item-value="name"
          class="bg-surface rounded"
        >
          <template v-slot:item.name="{ item }">
            <span class="text-body-1 font-weight-medium">{{ item.name }}</span>
          </template>

          <template v-slot:item.schedule="{ item }">
            <code class="text-body-2">{{ item.schedule }}</code>
          </template>

          <template v-slot:item.suspend="{ item }">
            <v-chip
              :color="item.suspend ? 'warning' : 'success'"
              size="small"
              variant="tonal"
            >
              {{ item.suspend ? "Yes" : "No" }}
            </v-chip>
          </template>

          <template v-slot:item.active_count="{ item }">
            <span class="text-body-2">{{ item.active_count }}</span>
          </template>

          <template v-slot:item.last_schedule_time="{ item }">
            <span class="text-body-2 text-secondary">
              {{ formatTimestamp(item.last_schedule_time) }}
            </span>
          </template>

          <template v-slot:item.created_at="{ item }">
            <span class="text-body-2 text-secondary">
              {{ formatAge(item.created_at) }}
            </span>
          </template>

          <template v-slot:no-data>
            <div class="text-center py-8 text-secondary">
              <v-icon icon="mdi-clock-outline" size="48" class="mb-2" />
              <div>No cronjobs in this namespace</div>
            </div>
          </template>
        </v-data-table>
      </v-window-item>

      <!-- Secrets tab -->
      <v-window-item value="secrets">
        <v-alert v-if="secretsError" type="error" class="mb-4" closable>
          {{ secretsError }}
        </v-alert>

        <v-data-table
          v-if="ns.selected"
          :items="secrets"
          :headers="secretHeaders"
          :loading="secretsLoading"
          item-value="name"
          class="bg-surface rounded"
        >
          <template v-slot:item.name="{ item }">
            <span class="text-body-1 font-weight-medium">{{ item.name }}</span>
          </template>

          <template v-slot:item.type="{ item }">
            <v-chip size="small" variant="tonal" color="secondary">
              {{ item.type }}
            </v-chip>
          </template>

          <template v-slot:item.keys="{ item }">
            <div class="d-flex flex-wrap ga-1">
              <v-chip
                v-for="k in item.keys"
                :key="k"
                size="x-small"
                variant="outlined"
              >
                {{ k }}
              </v-chip>
              <span v-if="item.keys.length === 0" class="text-caption text-secondary">
                (no data)
              </span>
            </div>
          </template>

          <template v-slot:item.created_at="{ item }">
            <span class="text-body-2 text-secondary">
              {{ formatAge(item.created_at) }}
            </span>
          </template>

          <template v-slot:item.actions="{ item }">
            <div class="d-flex ga-2">
              <v-btn
                size="small"
                variant="text"
                icon="mdi-eye"
                @click="openViewSecret(item.name)"
              />
              <v-btn
                size="small"
                variant="text"
                color="error"
                icon="mdi-delete"
                @click="deleteSecret(item.name)"
              />
            </div>
          </template>

          <template v-slot:no-data>
            <div class="text-center py-8 text-secondary">
              <v-icon icon="mdi-key-variant" size="48" class="mb-2" />
              <div>No secrets in this namespace</div>
            </div>
          </template>
        </v-data-table>
      </v-window-item>

      <!-- ConfigMaps tab -->
      <v-window-item value="configmaps">
        <v-alert v-if="configmapsError" type="error" class="mb-4" closable>
          {{ configmapsError }}
        </v-alert>

        <v-data-table
          v-if="ns.selected"
          :items="configmaps"
          :headers="cmHeaders"
          :loading="configmapsLoading"
          item-value="name"
          class="bg-surface rounded"
        >
          <template v-slot:item.name="{ item }">
            <span class="text-body-1 font-weight-medium">{{ item.name }}</span>
          </template>

          <template v-slot:item.keys="{ item }">
            <div class="d-flex flex-wrap ga-1">
              <v-chip
                v-for="k in item.keys"
                :key="k"
                size="x-small"
                variant="outlined"
              >
                {{ k }}
              </v-chip>
              <span v-if="item.keys.length === 0" class="text-caption text-secondary">
                (empty)
              </span>
            </div>
          </template>

          <template v-slot:item.created_at="{ item }">
            <span class="text-body-2 text-secondary">
              {{ formatAge(item.created_at) }}
            </span>
          </template>

          <template v-slot:item.actions="{ item }">
            <div class="d-flex ga-2">
              <v-btn
                size="small"
                variant="text"
                icon="mdi-pencil"
                @click="openEditCm(item.name)"
              />
              <v-btn
                size="small"
                variant="text"
                color="error"
                icon="mdi-delete"
                @click="deleteCm(item.name)"
              />
            </div>
          </template>

          <template v-slot:no-data>
            <div class="text-center py-8 text-secondary">
              <v-icon icon="mdi-file-cog-outline" size="48" class="mb-2" />
              <div>No configmaps in this namespace</div>
            </div>
          </template>
        </v-data-table>
      </v-window-item>
    </v-window>

    <!-- Create Secret Dialog -->
    <v-dialog v-model="showCreateSecret" max-width="720" persistent>
      <v-card>
        <v-card-title>Create Secret</v-card-title>
        <v-card-text>
          <v-alert v-if="secretDialogError" type="error" class="mb-4" closable>
            {{ secretDialogError }}
          </v-alert>
          <v-form @submit.prevent="submitSecret">
            <v-text-field
              v-model="secretName"
              label="Name"
              :rules="nameRules"
              autofocus
              required
            />
            <v-text-field
              v-model="secretType"
              label="Type"
              hint="e.g. Opaque, kubernetes.io/dockerconfigjson"
              persistent-hint
              class="mb-3"
            />

            <div class="text-subtitle-2 mb-2">Data</div>
            <div
              v-for="(row, i) in secretEntries"
              :key="i"
              class="d-flex align-start ga-2 mb-2"
            >
              <v-text-field
                v-model="row.key"
                label="Key"
                density="compact"
                hide-details="auto"
                style="max-width: 240px"
              />
              <v-text-field
                v-model="row.value"
                label="Value"
                :type="row.revealed ? 'text' : 'password'"
                density="compact"
                hide-details="auto"
                :append-inner-icon="row.revealed ? 'mdi-eye-off' : 'mdi-eye'"
                @click:append-inner="row.revealed = !row.revealed"
              />
              <v-btn
                icon="mdi-close"
                size="small"
                variant="text"
                @click="removeSecretRow(i)"
              />
            </div>
            <v-btn
              size="small"
              variant="text"
              prepend-icon="mdi-plus"
              @click="addSecretRow"
            >
              Add key/value
            </v-btn>
          </v-form>
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn
            variant="text"
            :disabled="secretDialogSubmitting"
            @click="showCreateSecret = false"
          >
            Cancel
          </v-btn>
          <v-btn
            color="primary"
            variant="flat"
            :loading="secretDialogSubmitting"
            @click="submitSecret"
          >
            Create
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <!-- View Secret Dialog -->
    <v-dialog v-model="showViewSecret" max-width="720">
      <v-card v-if="viewingSecret">
        <v-card-title class="d-flex align-center">
          <span>{{ viewingSecret.name }}</span>
          <v-chip size="small" variant="tonal" color="secondary" class="ml-3">
            {{ viewingSecret.type }}
          </v-chip>
          <v-spacer />
          <v-btn
            size="small"
            variant="tonal"
            :prepend-icon="revealSecretValues ? 'mdi-eye-off' : 'mdi-eye'"
            @click="toggleRevealSecretValues"
          >
            {{ revealSecretValues ? "Hide values" : "Reveal values" }}
          </v-btn>
        </v-card-title>
        <v-card-text>
          <div v-if="viewingSecret.keys.length === 0" class="text-secondary text-center py-4">
            No data
          </div>
          <v-list v-else density="compact">
            <v-list-item
              v-for="k in viewingSecret.keys"
              :key="k"
              class="border-b"
            >
              <template v-slot:prepend>
                <v-chip size="small" variant="outlined" class="mr-3">
                  {{ k }}
                </v-chip>
              </template>
              <span
                v-if="revealSecretValues && viewingSecret.data"
                class="font-mono text-body-2"
                style="word-break: break-all"
              >
                {{ viewingSecret.data[k] ?? "" }}
              </span>
              <span v-else class="text-secondary">••••••••</span>
            </v-list-item>
          </v-list>
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="showViewSecret = false">Close</v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <!-- Create/Edit ConfigMap Dialog -->
    <v-dialog v-model="showCreateCm" max-width="720" persistent>
      <v-card>
        <v-card-title>
          {{ cmDialogMode === "create" ? "Create ConfigMap" : `Edit ${cmName}` }}
        </v-card-title>
        <v-card-text>
          <v-alert v-if="cmDialogError" type="error" class="mb-4" closable>
            {{ cmDialogError }}
          </v-alert>
          <v-form @submit.prevent="submitCm">
            <v-text-field
              v-model="cmName"
              label="Name"
              :rules="nameRules"
              :readonly="cmDialogMode === 'edit'"
              autofocus
              required
            />

            <div class="text-subtitle-2 mb-2">Data</div>
            <div
              v-for="(row, i) in cmEntries"
              :key="i"
              class="d-flex align-start ga-2 mb-2"
            >
              <v-text-field
                v-model="row.key"
                label="Key"
                density="compact"
                hide-details="auto"
                style="max-width: 240px"
              />
              <v-textarea
                v-model="row.value"
                label="Value"
                density="compact"
                hide-details="auto"
                rows="1"
                auto-grow
              />
              <v-btn
                icon="mdi-close"
                size="small"
                variant="text"
                @click="removeCmRow(i)"
              />
            </div>
            <v-btn
              size="small"
              variant="text"
              prepend-icon="mdi-plus"
              @click="addCmRow"
            >
              Add key/value
            </v-btn>
          </v-form>
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn
            variant="text"
            :disabled="cmDialogSubmitting"
            @click="showCreateCm = false"
          >
            Cancel
          </v-btn>
          <v-btn
            color="primary"
            variant="flat"
            :loading="cmDialogSubmitting"
            @click="submitCm"
          >
            {{ cmDialogMode === "create" ? "Create" : "Save" }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <!-- Deployment row action dialogs -->
    <ConfirmDialog
      v-model="showRowRestartDialog"
      title="Restart Deployment"
      :message="`This will trigger a rolling restart of '${rowActionTarget}'.`"
      confirm-text="Restart"
      confirm-color="warning"
      :loading="rowActionLoading"
      @confirm="handleRowRestart"
    />

    <ConfirmDialog
      v-model="showRowDeleteDialog"
      title="Delete Deployment"
      :message="`Are you sure you want to delete '${rowActionTarget}'? This action cannot be undone.`"
      confirm-text="Delete"
      :confirm-input="rowActionTarget ?? ''"
      :loading="rowActionLoading"
      @confirm="handleRowDelete"
    />

    <v-dialog v-model="showRowScaleDialog" max-width="420">
      <v-card>
        <v-card-title>Scale Deployment</v-card-title>
        <v-card-text>
          <div class="mb-3">
            Set replica count for <strong>{{ rowActionTarget }}</strong>:
          </div>
          <v-text-field
            v-model.number="rowScaleReplicas"
            label="Replicas"
            type="number"
            min="0"
            density="compact"
            hide-details
            autofocus
          />
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn
            variant="text"
            :disabled="rowActionLoading"
            @click="showRowScaleDialog = false"
          >
            Cancel
          </v-btn>
          <v-btn
            color="primary"
            variant="flat"
            :loading="rowActionLoading"
            @click="handleRowScale"
          >
            Scale
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>
