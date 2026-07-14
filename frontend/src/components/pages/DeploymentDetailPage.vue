<script setup lang="ts">
import { ref, computed, watch } from "vue";
import { useRouter } from "vue-router";
import { deploymentsApi } from "@/api/deployments";
import { ingressesApi } from "@/api/ingresses";
import { usePolling } from "@/composables/usePolling";
import type {
  ContainerStatusSummary,
  DeploymentDetailResponse,
  CreateDeploymentRequest,
} from "@/types/api";
import StatusChip from "@/components/common/StatusChip.vue";
import ReplicaGauge from "@/components/views/deployment/ReplicaGauge.vue";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import DeploymentForm from "@/components/views/deployment/DeploymentForm.vue";
import PodTable from "@/components/views/pod/PodTable.vue";
import LogViewer from "@/components/common/LogViewer.vue";
import EventsFeed from "@/components/common/EventsFeed.vue";
import TerminalDialog from "@/components/common/TerminalDialog.vue";
import PortForwardDialog from "@/components/common/PortForwardDialog.vue";
import MetricSparkline from "@/components/common/MetricSparkline.vue";
import { usePodMetrics } from "@/composables/useResourceMetrics";
import MonitorSettingsCard from "@/components/views/deployment/MonitorSettingsCard.vue";
import AutoscalingCard from "@/components/views/deployment/AutoscalingCard.vue";
import GitOpsCard from "@/components/views/deployment/GitOpsCard.vue";
import YamlViewer from "@/components/common/YamlViewer.vue";
import { formatAge } from "@/utils/format";
import YamlEditor from "@/components/common/YamlEditor.vue";
import AddonsCard from "@/components/views/deployment/AddonsCard.vue";
import SidecarManager from "@/components/views/deployment/SidecarManager.vue";
import MetricsDetailCard from "@/components/views/deployment/MetricsDetailCard.vue";

const props = defineProps<{
  namespace: string;
  name: string;
}>();

const router = useRouter();
const detail = ref<DeploymentDetailResponse | null>(null);
const loading = ref(true);
const error = ref<string | null>(null);
const actionLoading = ref(false);

const showDeleteDialog = ref(false);
const showTerminalDialog = ref(false);
const showPortForwardDialog = ref(false);
const portForwardPod = ref<string | null>(null);
const terminalPod = ref<string | null>(null);
const showRestartDialog = ref(false);
const showEditDialog = ref(false);
const showMetricsDetail = ref(false);

// YAML view/edit state
const showYamlViewDialog = ref(false);
const yamlViewContent = ref<string>("");
const yamlViewLoading = ref(false);
const yamlViewError = ref<string | null>(null);

const editMode = ref<"form" | "yaml">("form");
const yamlEditContent = ref<string>("");
const yamlEditLoading = ref(false);
const yamlEditError = ref<string | null>(null);

// Log viewer state
const selectedLogPod = ref<string | null>(null);
const selectedLogContainer = ref<string | undefined>(undefined);
const logTailLines = ref<number | undefined>(undefined);

const podNames = computed(() =>
  detail.value?.pods.map((p) => p.name) ?? [],
);

const containerNames = computed(() => {
  if (!selectedLogPod.value || !detail.value) return [];
  const pod = detail.value.pods.find((p) => p.name === selectedLogPod.value);
  return pod?.container_statuses.map((c) => c.name) ?? [];
});

// Unique container list across all pods for SidecarManager.
// Order is taken from the first pod's container_statuses, which the backend
// emits in pod spec order (primary container at index 0). Additional
// containers seen only in later pods are appended.
const sidecarContainers = computed<ContainerStatusSummary[]>(() => {
  if (!detail.value || detail.value.pods.length === 0) return [];
  const ordered: ContainerStatusSummary[] = [];
  const seen = new Set<string>();
  for (const pod of detail.value.pods) {
    for (const c of pod.container_statuses) {
      if (!seen.has(c.name)) {
        seen.add(c.name);
        ordered.push(c);
      }
    }
  }
  return ordered;
});

watch(podNames, (names) => {
  if (!selectedLogPod.value && names.length > 0) {
    selectedLogPod.value = names[0];
  }
}, { immediate: true });

watch(selectedLogPod, () => {
  const names = containerNames.value;
  selectedLogContainer.value = names.length > 1 ? names[0] : undefined;
});

// Ingress state
const showIngressDialog = ref(false);
const editingIngressName = ref<string | null>(null);
const deleteIngressName = ref<string | null>(null);
const ingressForm = ref({
  name: "",
  host: "",
  path: "/",
  pathType: "Prefix",
  port: 80,
  ingressClass: "traefik",
});

const fetchDetail = async () => {
  try {
    detail.value = await deploymentsApi.get(props.namespace, props.name);
    loading.value = false;
  } catch (e) {
    error.value =
      e instanceof Error ? e.message : "Failed to fetch deployment";
    loading.value = false;
  }
};

usePolling(fetchDetail, 5000);

// Build a label selector from the deployment\'s labels so metrics-server
// only returns pods that belong to this deployment. `app.kubernetes.io/name`
// or the plain `app` label is what most deployments carry -- fall back to
// deployment name -> pod-template-hash matching via `app=<name>` when
// nothing else is on the object.
const namespaceRef = computed(() => props.namespace);
const labelSelector = computed<string | undefined>(() => {
  const labels = detail.value?.labels ?? {};
  const appName =
    labels["app.kubernetes.io/name"] ??
    labels["app"] ??
    props.name;
  return `app=${appName}`;
});
const {
  series: podMetricsSeries,
  latest: podMetricsLatest,
  unavailableReason: metricsUnavailable,
  loading: metricsLoading,
} = usePodMetrics(namespaceRef, labelSelector);

const totalPodCpuMillicores = computed(() => {
  let total = 0;
  for (const p of podMetricsLatest.value.values()) total += p.total_cpu_millicores;
  return total;
});
const totalPodMemBytes = computed(() => {
  let total = 0;
  for (const p of podMetricsLatest.value.values()) total += p.total_memory_bytes;
  return total;
});
// Aggregate deployment-level ring buffers (sum across all pods per sample
// tick). Pods share the same polling cadence, so we can align by index.
const aggregateCpuSeries = computed<number[]>(() => {
  const buffers = Array.from(podMetricsSeries.value.values()).map((s) => s.samples);
  if (buffers.length === 0) return [];
  const maxLen = Math.max(...buffers.map((b) => b.length));
  const out: number[] = [];
  for (let i = 0; i < maxLen; i++) {
    let sum = 0;
    for (const buf of buffers) {
      const s = buf[buf.length - maxLen + i];
      if (s) sum += s.cpuMillicores;
    }
    out.push(sum);
  }
  return out;
});
const aggregateMemSeries = computed<number[]>(() => {
  const buffers = Array.from(podMetricsSeries.value.values()).map((s) => s.samples);
  if (buffers.length === 0) return [];
  const maxLen = Math.max(...buffers.map((b) => b.length));
  const out: number[] = [];
  for (let i = 0; i < maxLen; i++) {
    let sum = 0;
    for (const buf of buffers) {
      const s = buf[buf.length - maxLen + i];
      if (s) sum += s.memBytes;
    }
    out.push(sum);
  }
  return out;
});
const formatCpuTotal = (mcpu: number): string => {
  if (mcpu >= 1000) return `${(mcpu / 1000).toFixed(2)} cores`;
  return `${mcpu}m`;
};
const formatMemTotal = (bytes: number): string => {
  const mib = bytes / (1024 * 1024);
  if (mib >= 1024) return `${(mib / 1024).toFixed(1)} GiB`;
  return `${Math.round(mib)} MiB`;
};

const editInitialValues = computed(
  (): Partial<CreateDeploymentRequest> | undefined => {
    if (!detail.value) return undefined;
    const d = detail.value;
    return {
      name: d.name,
      image: d.image,
      replicas: d.replicas.desired,
      env: d.env
        .filter((e) => e.value !== null)
        .map((e) => ({ name: e.name, value: e.value! })),
      command: d.command,
      args: d.args,
      resource_requests: d.resource_requests ?? undefined,
      resource_limits: d.resource_limits ?? undefined,
      liveness_probe: d.liveness_probe as CreateDeploymentRequest["liveness_probe"],
      readiness_probe: d.readiness_probe as CreateDeploymentRequest["readiness_probe"],
      startup_probe: d.startup_probe as CreateDeploymentRequest["startup_probe"],
    };
  },
);

const handleDelete = async () => {
  actionLoading.value = true;
  try {
    await deploymentsApi.delete(props.namespace, props.name);
    showDeleteDialog.value = false;
    router.push({ name: "Deployments" });
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to delete";
  } finally {
    actionLoading.value = false;
  }
};

const handleRestart = async () => {
  actionLoading.value = true;
  try {
    await deploymentsApi.restart(props.namespace, props.name);
    showRestartDialog.value = false;
    await fetchDetail();
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to restart";
  } finally {
    actionLoading.value = false;
  }
};

// Replica scaling is exposed through the edit form's Replicas slider — the
// existing update path handles the replica count alongside the rest of the
// spec, so a dedicated scale endpoint call is unnecessary from the UI.
// Health-check probes are edited through the same form and included in the
// update payload here.
const handleEdit = async (values: CreateDeploymentRequest) => {
  actionLoading.value = true;
  try {
    await deploymentsApi.update(props.namespace, props.name, {
      image: values.image,
      replicas: values.replicas,
      env: values.env,
      command: values.command,
      args: values.args,
      resource_requests: values.resource_requests,
      resource_limits: values.resource_limits,
      liveness_probe: values.liveness_probe,
      readiness_probe: values.readiness_probe,
      startup_probe: values.startup_probe,
    });
    showEditDialog.value = false;
    await fetchDetail();
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to update";
  } finally {
    actionLoading.value = false;
  }
};

const openYamlView = async () => {
  showYamlViewDialog.value = true;
  yamlViewContent.value = "";
  yamlViewError.value = null;
  yamlViewLoading.value = true;
  try {
    yamlViewContent.value = await deploymentsApi.getYaml(
      props.namespace,
      props.name,
    );
  } catch (e) {
    yamlViewError.value =
      e instanceof Error ? e.message : "Failed to fetch YAML";
  } finally {
    yamlViewLoading.value = false;
  }
};

const openEditDialog = () => {
  editMode.value = "form";
  yamlEditContent.value = "";
  yamlEditError.value = null;
  showEditDialog.value = true;
};

const loadYamlForEdit = async () => {
  yamlEditError.value = null;
  yamlEditLoading.value = true;
  try {
    yamlEditContent.value = await deploymentsApi.getYaml(
      props.namespace,
      props.name,
    );
  } catch (e) {
    yamlEditError.value =
      e instanceof Error ? e.message : "Failed to fetch YAML";
  } finally {
    yamlEditLoading.value = false;
  }
};

watch(
  () => [showEditDialog.value, editMode.value] as const,
  ([open, mode]) => {
    if (open && mode === "yaml" && !yamlEditContent.value && !yamlEditLoading.value) {
      void loadYamlForEdit();
    }
  },
);

const handleYamlSave = async (yaml: string) => {
  actionLoading.value = true;
  yamlEditError.value = null;
  try {
    await deploymentsApi.updateYaml(props.namespace, props.name, yaml);
    showEditDialog.value = false;
    await fetchDetail();
  } catch (e) {
    yamlEditError.value =
      e instanceof Error ? e.message : "Failed to update from YAML";
  } finally {
    actionLoading.value = false;
  }
};

const handleSidecarChanged = async (updated?: DeploymentDetailResponse) => {
  if (updated) {
    detail.value = updated;
  } else {
    await fetchDetail();
  }
};

const openCreateIngress = () => {
  editingIngressName.value = null;
  ingressForm.value = {
    name: `${props.name}-ingress`,
    host: "",
    path: "/",
    pathType: "Prefix",
    port: 80,
    ingressClass: "traefik",
  };
  showIngressDialog.value = true;
};

const openEditIngress = async (ingressName: string) => {
  actionLoading.value = true;
  try {
    const ing = await ingressesApi.get(props.namespace, ingressName);
    const firstRule = ing.rules[0];
    const firstPath = firstRule?.paths[0];
    editingIngressName.value = ingressName;
    ingressForm.value = {
      name: ing.name,
      host: firstRule?.host ?? "",
      path: firstPath?.path ?? "/",
      pathType: firstPath?.path_type ?? "Prefix",
      port: firstPath?.service_port ?? 80,
      ingressClass: ing.ingress_class ?? "traefik",
    };
    showIngressDialog.value = true;
  } catch (e) {
    error.value =
      e instanceof Error ? e.message : "Failed to fetch ingress";
  } finally {
    actionLoading.value = false;
  }
};

const handleSaveIngress = async () => {
  actionLoading.value = true;
  const payload = {
    name: ingressForm.value.name,
    host: ingressForm.value.host || undefined,
    paths: [
      {
        path: ingressForm.value.path,
        path_type: ingressForm.value.pathType,
        service_name: props.name,
        service_port: ingressForm.value.port,
      },
    ],
    ingress_class: ingressForm.value.ingressClass || undefined,
  };
  try {
    if (editingIngressName.value) {
      await ingressesApi.update(
        props.namespace,
        editingIngressName.value,
        payload,
      );
    } else {
      await ingressesApi.create(props.namespace, payload);
    }
    showIngressDialog.value = false;
    editingIngressName.value = null;
    await fetchDetail();
  } catch (e) {
    error.value =
      e instanceof Error ? e.message : "Failed to save ingress";
  } finally {
    actionLoading.value = false;
  }
};

const handleDeleteIngress = async () => {
  if (!deleteIngressName.value) return;
  actionLoading.value = true;
  try {
    await ingressesApi.delete(props.namespace, deleteIngressName.value);
    deleteIngressName.value = null;
    await fetchDetail();
  } catch (e) {
    error.value =
      e instanceof Error ? e.message : "Failed to delete ingress";
  } finally {
    actionLoading.value = false;
  }
};
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <v-btn
        icon="mdi-arrow-left"
        variant="text"
        @click="router.push({ name: 'Deployments' })"
      />
      <div class="ml-2">
        <h2 class="text-h5">{{ name }}</h2>
        <span class="text-caption text-secondary">{{ namespace }}</span>
      </div>
      <v-spacer />
      <div v-if="detail" class="d-flex ga-2">
        <v-btn
          icon="mdi-refresh"
          variant="text"
          size="small"
          :loading="loading"
          @click="fetchDetail"
        />
        <v-btn
          variant="outlined"
          size="small"
          prepend-icon="mdi-pencil"
          @click="openEditDialog"
        >
          Edit
        </v-btn>
        <v-btn
          variant="outlined"
          size="small"
          prepend-icon="mdi-file-code-outline"
          @click="openYamlView"
        >
          View YAML
        </v-btn>
        <v-btn
          variant="outlined"
          size="small"
          prepend-icon="mdi-restart"
          @click="showRestartDialog = true"
        >
          Restart
        </v-btn>
        <v-btn
          variant="outlined"
          size="small"
          color="error"
          prepend-icon="mdi-delete"
          @click="showDeleteDialog = true"
        >
          Delete
        </v-btn>
      </div>
    </div>

    <v-alert v-if="error" type="error" class="mb-4" closable>
      {{ error }}
    </v-alert>

    <v-progress-linear v-if="loading" indeterminate color="primary" />

    <template v-if="detail">
      <!-- Status Overview (with inline resource sparklines) -->
      <v-card class="mb-4 pa-4">
        <div class="d-flex align-center ga-4 flex-wrap">
          <StatusChip :status="detail.status" />
          <ReplicaGauge :replicas="detail.replicas" />
          <v-chip variant="outlined" size="small">
            <v-icon icon="mdi-docker" start size="small" />
            {{ detail.image }}
          </v-chip>
          <v-chip variant="outlined" size="small" color="secondary">
            {{ formatAge(detail.created_at, { suffix: " ago" }) }}
          </v-chip>
          <template v-if="!metricsUnavailable">
            <v-divider vertical class="mx-1" />
            <MetricSparkline
              label="CPU"
              :data="aggregateCpuSeries"
              :latest-formatted="formatCpuTotal(totalPodCpuMillicores)"
              color="primary"
              :width="96"
              :height="22"
              fill
            />
            <MetricSparkline
              label="Mem"
              :data="aggregateMemSeries"
              :latest-formatted="formatMemTotal(totalPodMemBytes)"
              color="secondary"
              :width="96"
              :height="22"
              fill
            />
            <v-btn
              variant="text"
              size="x-small"
              :prepend-icon="showMetricsDetail ? 'mdi-chevron-up' : 'mdi-chart-line'"
              @click="showMetricsDetail = !showMetricsDetail"
            >
              {{ showMetricsDetail ? "Hide details" : "View details" }}
            </v-btn>
          </template>
        </div>
        <div
          v-if="detail.liveness_probe || detail.readiness_probe || detail.startup_probe"
          class="d-flex align-center ga-2 mt-3"
        >
          <span class="text-caption text-secondary mr-1">Probes:</span>
          <v-chip
            v-if="detail.liveness_probe"
            size="x-small"
            color="info"
            variant="flat"
          >
            <v-icon icon="mdi-heart-pulse" start size="x-small" />
            Liveness ({{ detail.liveness_probe.probe_type }})
          </v-chip>
          <v-chip
            v-if="detail.readiness_probe"
            size="x-small"
            color="info"
            variant="flat"
          >
            <v-icon icon="mdi-check-network" start size="x-small" />
            Readiness ({{ detail.readiness_probe.probe_type }})
          </v-chip>
          <v-chip
            v-if="detail.startup_probe"
            size="x-small"
            color="info"
            variant="flat"
          >
            <v-icon icon="mdi-rocket-launch" start size="x-small" />
            Startup ({{ detail.startup_probe.probe_type }})
          </v-chip>
        </div>
        <v-alert
          v-if="metricsUnavailable"
          type="info"
          density="compact"
          variant="tonal"
          class="mt-3"
        >
          {{ metricsUnavailable }}
        </v-alert>
      </v-card>

      <!-- Metrics detail (expandable from sparklines above) -->
      <v-expand-transition>
        <MetricsDetailCard
          v-if="showMetricsDetail && !metricsUnavailable"
          :series="podMetricsSeries"
          :latest="podMetricsLatest"
          :loading="metricsLoading"
          @close="showMetricsDetail = false"
        />
      </v-expand-transition>

      <!-- Ingresses -->
      <v-card class="mb-4">
        <v-card-title class="d-flex align-center">
          <span class="text-subtitle-1">Ingresses</span>
          <v-spacer />
          <v-btn
            size="small"
            variant="text"
            prepend-icon="mdi-plus"
            @click="openCreateIngress"
          >
            Add
          </v-btn>
        </v-card-title>

        <v-table v-if="detail.ingresses.length > 0" density="compact">
          <thead>
            <tr>
              <th>Name</th>
              <th>Hosts</th>
              <th>Class</th>
              <th>Addresses</th>
              <th style="width: 90px"></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="ing in detail.ingresses" :key="ing.name">
              <td class="font-weight-medium">{{ ing.name }}</td>
              <td>
                <v-chip
                  v-for="host in ing.hosts"
                  :key="host"
                  size="x-small"
                  variant="flat"
                  color="primary"
                  class="mr-1"
                >
                  {{ host }}
                </v-chip>
                <span
                  v-if="ing.hosts.length === 0"
                  class="text-secondary"
                >
                  *
                </span>
              </td>
              <td class="text-caption text-secondary">
                {{ ing.ingress_class ?? "-" }}
              </td>
              <td class="text-caption text-secondary">
                {{ ing.addresses.join(", ") || "-" }}
              </td>
              <td class="d-flex ga-1 align-center">
                <v-btn
                  icon="mdi-pencil"
                  size="x-small"
                  variant="text"
                  @click="openEditIngress(ing.name)"
                />
                <v-btn
                  icon="mdi-delete"
                  size="x-small"
                  variant="text"
                  color="error"
                  @click="deleteIngressName = ing.name"
                />
              </td>
            </tr>
          </tbody>
        </v-table>

        <div
          v-else
          class="text-center py-4 text-secondary text-body-2"
        >
          No ingresses configured
        </div>
      </v-card>

      <!-- Monitoring -->
      <MonitorSettingsCard :namespace="namespace" :deployment-name="name" />

      <!-- Autoscaling -->
      <AutoscalingCard :namespace="namespace" :deployment-name="name" />

      <!-- GitOps -->
      <GitOpsCard :namespace="namespace" :deployment-name="name" />

      <!-- Addons -->
      <AddonsCard
        :namespace="namespace"
        :deployment-name="name"
        :containers="sidecarContainers"
        @changed="fetchDetail"
      />

      <!-- Sidecar Containers -->
      <SidecarManager
        :namespace="namespace"
        :deployment-name="name"
        :containers="sidecarContainers"
        @changed="handleSidecarChanged"
      />

      <!-- Pods -->
      <v-card class="mb-4">
        <v-card-title class="text-subtitle-1">
          Pods ({{ detail.pods.length }})
        </v-card-title>
        <PodTable
          :pods="detail.pods"
          :namespace="namespace"
          :metrics-series="metricsUnavailable ? undefined : podMetricsSeries"
          :latest-usage="metricsUnavailable ? undefined : podMetricsLatest"
        />
      </v-card>

      <!-- Conditions -->
      <v-card v-if="detail.conditions.length > 0" class="mb-4">
        <v-card-title class="text-subtitle-1">Conditions</v-card-title>
        <v-table density="compact">
          <thead>
            <tr>
              <th>Type</th>
              <th>Status</th>
              <th>Reason</th>
              <th>Message</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="c in detail.conditions" :key="c.condition_type">
              <td>{{ c.condition_type }}</td>
              <td>
                <v-icon
                  :icon="c.status === 'True' ? 'mdi-check' : 'mdi-close'"
                  :color="c.status === 'True' ? 'success' : 'error'"
                  size="small"
                />
              </td>
              <td class="text-caption">{{ c.reason ?? "-" }}</td>
              <td class="text-caption text-secondary">
                {{ c.message ?? "-" }}
              </td>
            </tr>
          </tbody>
        </v-table>
      </v-card>

      <!-- Events -->
      <EventsFeed :namespace="namespace" :involved-object="name" title="Events" />

      <!-- Logs -->
      <v-card v-if="detail.pods.length > 0" class="mb-4">
        <v-card-title class="d-flex align-center">
          <span class="text-subtitle-1">Logs</span>
          <v-spacer />
          <v-select
            v-model="selectedLogPod"
            :items="podNames"
            label="Pod"
            density="compact"
            variant="outlined"
            hide-details
            style="max-width: 320px"
            class="mr-2"
          />
          <v-select
            v-if="containerNames.length > 1"
            v-model="selectedLogContainer"
            :items="containerNames"
            label="Container"
            density="compact"
            variant="outlined"
            hide-details
            clearable
            style="max-width: 180px"
            class="mr-2"
          />
          <v-select
            v-model="logTailLines"
            :items="[
              { title: 'Full history', value: undefined },
              { title: 'Last 100', value: 100 },
              { title: 'Last 500', value: 500 },
              { title: 'Last 1000', value: 1000 },
            ]"
            label="History"
            density="compact"
            variant="outlined"
            hide-details
            style="max-width: 150px"
          />
        </v-card-title>

        <LogViewer
          v-if="selectedLogPod"
          :namespace="namespace"
          :pod-name="selectedLogPod"
          :container="selectedLogContainer"
          :tail-lines="logTailLines"
        />
      </v-card>
    </template>

    <!-- Deployment Dialogs -->
    <ConfirmDialog
      v-model="showDeleteDialog"
      title="Delete Deployment"
      :message="`Are you sure you want to delete '${name}'? This action cannot be undone.`"
      confirm-text="Delete"
      :require-type-confirm="name"
      :loading="actionLoading"
      @confirm="handleDelete"
    />

    <ConfirmDialog
      v-model="showRestartDialog"
      title="Restart Deployment"
      :message="`This will trigger a rolling restart of '${name}'.`"
      confirm-text="Restart"
      confirm-color="warning"
      :loading="actionLoading"
      @confirm="handleRestart"
    />

    <v-dialog v-model="showEditDialog" max-width="900">
      <v-card class="pa-4">
        <v-card-title class="d-flex align-center">
          <span>Edit Deployment</span>
          <v-spacer />
          <v-btn-toggle
            v-model="editMode"
            mandatory
            density="compact"
            variant="outlined"
            divided
          >
            <v-btn value="form" size="small" prepend-icon="mdi-form-select">
              Form
            </v-btn>
            <v-btn value="yaml" size="small" prepend-icon="mdi-file-code-outline">
              YAML
            </v-btn>
          </v-btn-toggle>
        </v-card-title>
        <v-card-text>
          <template v-if="editMode === 'form'">
            <DeploymentForm
              :initial-values="editInitialValues"
              is-edit
              :loading="actionLoading"
              @submit="handleEdit"
            />
          </template>
          <template v-else>
            <v-alert v-if="yamlEditError" type="error" density="compact" class="mb-2">
              {{ yamlEditError }}
            </v-alert>
            <div v-if="yamlEditLoading" class="text-center py-6 text-secondary">
              <v-progress-circular indeterminate color="primary" />
              <div class="mt-2 text-body-2">Loading YAML...</div>
            </div>
            <YamlEditor
              v-else
              :initial-yaml="yamlEditContent"
              :loading="actionLoading"
              @save="handleYamlSave"
              @cancel="showEditDialog = false"
            />
          </template>
        </v-card-text>
      </v-card>
    </v-dialog>

    <!-- View YAML Dialog -->
    <v-dialog v-model="showYamlViewDialog" max-width="900">
      <v-card class="pa-4">
        <v-card-title class="d-flex align-center">
          <span>{{ name }} — YAML</span>
          <v-spacer />
          <v-btn
            icon="mdi-close"
            variant="text"
            size="small"
            @click="showYamlViewDialog = false"
          />
        </v-card-title>
        <v-card-text>
          <v-alert v-if="yamlViewError" type="error" density="compact" class="mb-2">
            {{ yamlViewError }}
          </v-alert>
          <div v-if="yamlViewLoading" class="text-center py-6 text-secondary">
            <v-progress-circular indeterminate color="primary" />
            <div class="mt-2 text-body-2">Loading YAML...</div>
          </div>
          <YamlViewer v-else :yaml="yamlViewContent" />
        </v-card-text>
      </v-card>
    </v-dialog>

    <!-- Ingress Create/Edit Dialog -->
    <v-dialog v-model="showIngressDialog" max-width="520">
      <v-card>
        <v-card-title>
          {{ editingIngressName ? "Edit Ingress" : "Add Ingress" }}
        </v-card-title>
        <v-card-text>
          <v-text-field
            v-model="ingressForm.name"
            label="Ingress Name"
            :disabled="!!editingIngressName"
            class="mb-2"
          />
          <v-text-field
            v-model="ingressForm.host"
            label="Hostname"
            placeholder="myapp.example.com"
            class="mb-2"
          />
          <v-row dense>
            <v-col cols="5">
              <v-text-field
                v-model="ingressForm.path"
                label="Path"
              />
            </v-col>
            <v-col cols="4">
              <v-select
                v-model="ingressForm.pathType"
                :items="['Prefix', 'Exact']"
                label="Type"
              />
            </v-col>
            <v-col cols="3">
              <v-text-field
                v-model.number="ingressForm.port"
                label="Port"
                type="number"
              />
            </v-col>
          </v-row>
          <v-text-field
            v-model="ingressForm.ingressClass"
            label="Ingress Class"
            placeholder="traefik"
          />
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn
            variant="text"
            @click="showIngressDialog = false"
          >
            Cancel
          </v-btn>
          <v-btn
            color="primary"
            variant="flat"
            :loading="actionLoading"
            @click="handleSaveIngress"
          >
            {{ editingIngressName ? "Save" : "Create" }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <ConfirmDialog
      :model-value="!!deleteIngressName"
      title="Delete Ingress"
      :message="`Delete ingress '${deleteIngressName}'? This will remove the routing rule.`"
      confirm-text="Delete"
      :loading="actionLoading"
      @update:model-value="deleteIngressName = null"
      @confirm="handleDeleteIngress"
    />
    <PortForwardDialog
      v-if="portForwardPod"
      v-model="showPortForwardDialog"
      :namespace="namespace"
      :pod-name="portForwardPod"
    />

    <TerminalDialog
      v-if="terminalPod"
      v-model="showTerminalDialog"
      :namespace="namespace"
      :pod-name="terminalPod"
    />
  </div>
</template>
