<script setup lang="ts">
import { ref, computed, watch } from "vue";
import { useRouter } from "vue-router";
import { deploymentsApi } from "@/api/deployments";
import { ingressesApi } from "@/api/ingresses";
import { ingressClassesApi } from "@/api/ingresses";
import type { IngressClassInfo } from "@/api/ingresses";
import { autoscalingApi } from "@/api/autoscaling";
import { usePolling } from "@/composables/usePolling";
import type {
  ContainerStatusSummary,
  DeploymentDetailResponse,
  CreateDeploymentRequest,
  HpaResponse,
  HpaConfigRequest,
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
import { useFeatures } from "@/composables/useFeatures";
import GitOpsCard from "@/components/views/deployment/GitOpsCard.vue";
import YamlViewer from "@/components/common/YamlViewer.vue";
import { formatAge } from "@/utils/format";
import YamlEditor from "@/components/common/YamlEditor.vue";
import AddonsCard from "@/components/views/deployment/AddonsCard.vue";
import SidecarManager from "@/components/views/deployment/SidecarManager.vue";
import MetricsDetailCard from "@/components/views/deployment/MetricsDetailCard.vue";
import HistoryCard from "@/components/views/deployment/HistoryCard.vue";

const props = defineProps<{
  namespace: string;
  name: string;
}>();

const router = useRouter();
const { features } = useFeatures();
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
const showDiagDrawer = ref(false);

// Autoscaling (HPA) state
const hpa = ref<HpaResponse | null>(null);
const hpaError = ref<string | null>(null);
const hpaEnabled = computed(() => hpa.value !== null);

// Autoscaling form state for edit dialog
const hpaForm = ref({
  enabled: false,
  minReplicas: 1,
  maxReplicas: 3,
  cpuEnabled: true,
  cpuTarget: 80,
  memoryEnabled: false,
  memoryTarget: 80,
});

const fetchHpa = async () => {
  try {
    hpa.value = await autoscalingApi.get(props.namespace, props.name);
    hpaError.value = null;
  } catch (e) {
    hpaError.value =
      e instanceof Error ? e.message : "Failed to fetch autoscaling config";
  }
};

usePolling(fetchHpa, 5000);

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
const ingressClasses = ref<IngressClassInfo[]>([]);
const ingressClassNames = ref<string[]>([]);
const ingressForm = ref({
  name: "",
  host: "",
  path: "/",
  pathType: "Prefix",
  port: 80,
  ingressClass: "",
});

const fetchIngressClasses = async () => {
  try {
    const res = await ingressClassesApi.list();
    ingressClasses.value = res.classes;
    ingressClassNames.value = res.classes.map((c) => c.name);
  } catch {
    // Non-critical; fall back to manual entry via combobox
  }
};

// Derive the first container port from the deployment's probe config or
// fall back to the default 80. The deployment detail doesn't expose raw
// containerPort, so probes are the best signal available.
const firstContainerPort = computed<number>(() => {
  if (!detail.value) return 80;
  const probePort =
    detail.value.liveness_probe?.port ??
    detail.value.readiness_probe?.port ??
    detail.value.startup_probe?.port;
  return probePort ?? 80;
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

// Diagnostic drill-down: conditions and failing pod info for the status drawer.
const failedConditions = computed(() => {
  if (!detail.value) return [];
  return detail.value.conditions.filter((c) => c.status !== "True");
});

const failingPodStatuses = computed(() => {
  if (!detail.value) return [];
  const statuses: { podName: string; containerName: string; state: string; reason: string; summary: string }[] = [];
  for (const pod of detail.value.pods) {
    for (const cs of pod.container_statuses) {
      if (cs.state_reason) {
        statuses.push({
          podName: pod.name,
          containerName: cs.name,
          state: cs.state,
          reason: cs.state_reason,
          summary: stateReasonToAdvice(cs.state_reason),
        });
      }
    }
  }
  return statuses;
});

function stateReasonToAdvice(reason: string): string {
  const map: Record<string, string> = {
    CrashLoopBackOff: "Your app is crashing on startup. Check the logs for stack traces or missing config.",
    ImagePullBackOff: "The container image could not be pulled. Verify the image name, tag, and registry credentials.",
    ErrImagePull: "Failed to pull the container image. Check the image reference and network connectivity.",
    OOMKilled: "The container was killed because it exceeded its memory limit. Increase the memory limit or fix a memory leak.",
    CreateContainerConfigError: "Invalid container configuration. Check environment variables, secrets, and config map references.",
    RunContainerError: "The container failed to start. Check the command, entrypoint, and volume mounts.",
    ContainerCannotRun: "The container cannot run. Verify the entrypoint binary exists and is executable.",
    Completed: "The container exited successfully. If this is unexpected, check if the process is meant to run as a long-lived service.",
    Error: "The container exited with an error. Check the logs for details.",
    InvalidImageName: "The image name is malformed. Verify the repository URL and tag format.",
  };
  return map[reason] ?? `Container is in '${reason}' state. Check the logs and events for more details.`;
}

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

    // Save or delete HPA alongside the deployment update
    if (hpaForm.value.enabled) {
      const body: HpaConfigRequest = {
        min_replicas: hpaForm.value.minReplicas,
        max_replicas: hpaForm.value.maxReplicas,
        target_cpu_utilization: hpaForm.value.cpuEnabled
          ? hpaForm.value.cpuTarget
          : undefined,
        target_memory_utilization: hpaForm.value.memoryEnabled
          ? hpaForm.value.memoryTarget
          : undefined,
      };
      hpa.value = await autoscalingApi.upsert(
        props.namespace,
        props.name,
        body,
      );
    } else if (hpaEnabled.value) {
      // Was enabled, user toggled it off -- delete
      await autoscalingApi.delete(props.namespace, props.name);
      hpa.value = null;
    }

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

  // Prefill autoscaling form from current HPA state
  if (hpa.value) {
    hpaForm.value = {
      enabled: true,
      minReplicas: hpa.value.min_replicas ?? 1,
      maxReplicas: hpa.value.max_replicas,
      cpuEnabled: hpa.value.target_cpu_utilization !== null,
      cpuTarget: hpa.value.target_cpu_utilization ?? 80,
      memoryEnabled: hpa.value.target_memory_utilization !== null,
      memoryTarget: hpa.value.target_memory_utilization ?? 80,
    };
  } else {
    hpaForm.value = {
      enabled: false,
      minReplicas: 1,
      maxReplicas: 3,
      cpuEnabled: true,
      cpuTarget: 80,
      memoryEnabled: false,
      memoryTarget: 80,
    };
  }

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

const openCreateIngress = async () => {
  editingIngressName.value = null;
  await fetchIngressClasses();
  const defaultClass =
    ingressClasses.value.find((c) => c.is_default)?.name ?? "";
  ingressForm.value = {
    name: `${props.name}-ingress`,
    host: "",
    path: "/",
    pathType: "Prefix",
    port: firstContainerPort.value,
    ingressClass: defaultClass,
  };
  showIngressDialog.value = true;
};

const openEditIngress = async (ingressName: string) => {
  actionLoading.value = true;
  try {
    await fetchIngressClasses();
    const ing = await ingressesApi.get(props.namespace, ingressName);
    const firstRule = ing.rules[0];
    const firstPath = firstRule?.paths[0];
    editingIngressName.value = ingressName;
    ingressForm.value = {
      name: ing.name,
      host: firstRule?.host ?? "",
      path: firstPath?.path ?? "/",
      pathType: firstPath?.path_type ?? "Prefix",
      port: firstPath?.service_port ?? firstContainerPort.value,
      ingressClass: ing.ingress_class ?? "",
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
          <StatusChip :status="detail.status" @click="showDiagDrawer = true" />
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
        <div
          v-if="hpaEnabled && hpa"
          class="d-flex align-center ga-2 mt-3"
        >
          <span class="text-caption text-secondary mr-1">HPA:</span>
          <v-chip size="x-small" variant="outlined">
            <v-icon icon="mdi-arrow-collapse" start size="x-small" />
            Min {{ hpa.min_replicas ?? "-" }}
          </v-chip>
          <v-chip size="x-small" variant="outlined">
            <v-icon icon="mdi-arrow-expand" start size="x-small" />
            Max {{ hpa.max_replicas }}
          </v-chip>
          <v-chip
            v-if="hpa.target_cpu_utilization !== null"
            size="x-small"
            variant="outlined"
            color="primary"
          >
            <v-icon icon="mdi-chip" start size="x-small" />
            CPU {{ hpa.current_cpu_utilization ?? "-" }}% / {{ hpa.target_cpu_utilization }}%
          </v-chip>
          <v-chip
            v-if="hpa.target_memory_utilization !== null"
            size="x-small"
            variant="outlined"
            color="secondary"
          >
            <v-icon icon="mdi-memory" start size="x-small" />
            Mem {{ hpa.current_memory_utilization ?? "-" }}% / {{ hpa.target_memory_utilization }}%
          </v-chip>
          <v-chip size="x-small" variant="flat" color="info">
            {{ hpa.current_replicas ?? "?" }} / {{ hpa.desired_replicas ?? "?" }} replicas
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

      <!-- Monitoring (gated by prometheus feature flag) -->
      <MonitorSettingsCard v-if="features?.prometheus" :namespace="namespace" :deployment-name="name" />

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

      <!-- Rollout History -->
      <HistoryCard
        :namespace="namespace"
        :name="name"
        @rolled-back="fetchDetail"
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
      :confirm-input="name"
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

            <!-- Autoscaling section in edit dialog -->
            <v-divider class="my-4" />
            <div class="text-subtitle-2 mb-3 d-flex align-center">
              <v-icon icon="mdi-arrow-expand-vertical" size="small" class="mr-2" />
              Autoscaling
            </div>
            <v-alert v-if="hpaError" type="error" density="compact" class="mb-2" closable>
              {{ hpaError }}
            </v-alert>
            <v-switch
              v-model="hpaForm.enabled"
              label="Enable Horizontal Pod Autoscaler"
              density="compact"
              hide-details
              color="primary"
              class="mb-3"
            />
            <template v-if="hpaForm.enabled">
              <v-row dense>
                <v-col cols="6">
                  <v-text-field
                    v-model.number="hpaForm.minReplicas"
                    label="Min Replicas"
                    type="number"
                    min="1"
                    density="compact"
                    hide-details
                  />
                </v-col>
                <v-col cols="6">
                  <v-text-field
                    v-model.number="hpaForm.maxReplicas"
                    label="Max Replicas"
                    type="number"
                    :min="hpaForm.minReplicas"
                    density="compact"
                    hide-details
                  />
                </v-col>
              </v-row>
              <div class="text-body-2 mt-3 mb-2">Metric Targets</div>
              <div class="d-flex align-center ga-3 mb-2">
                <v-switch
                  v-model="hpaForm.cpuEnabled"
                  label="CPU"
                  density="compact"
                  hide-details
                  color="primary"
                  style="max-width: 120px"
                />
                <v-text-field
                  v-model.number="hpaForm.cpuTarget"
                  label="Target CPU %"
                  type="number"
                  min="1"
                  max="100"
                  density="compact"
                  hide-details
                  :disabled="!hpaForm.cpuEnabled"
                  suffix="%"
                  style="max-width: 180px"
                />
              </div>
              <div class="d-flex align-center ga-3">
                <v-switch
                  v-model="hpaForm.memoryEnabled"
                  label="Memory"
                  density="compact"
                  hide-details
                  color="secondary"
                  style="max-width: 120px"
                />
                <v-text-field
                  v-model.number="hpaForm.memoryTarget"
                  label="Target Memory %"
                  type="number"
                  min="1"
                  max="100"
                  density="compact"
                  hide-details
                  :disabled="!hpaForm.memoryEnabled"
                  suffix="%"
                  style="max-width: 180px"
                />
              </div>
              <v-alert
                v-if="hpaForm.enabled && !hpaForm.cpuEnabled && !hpaForm.memoryEnabled"
                type="warning"
                density="compact"
                variant="tonal"
                class="mt-3"
              >
                At least one metric target must be enabled.
              </v-alert>
            </template>
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
          <v-combobox
            v-model="ingressForm.ingressClass"
            :items="ingressClassNames"
            label="Ingress Class"
            placeholder="Select or type a class"
            clearable
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

    <!-- Diagnostic drill-down drawer (StatusChip click) -->
    <v-navigation-drawer
      v-model="showDiagDrawer"
      location="right"
      temporary
      width="480"
    >
      <v-card flat>
        <v-card-title class="d-flex align-center">
          <v-icon icon="mdi-stethoscope" class="mr-2" size="small" />
          Deployment Diagnostics
          <v-spacer />
          <v-btn icon="mdi-close" variant="text" size="small" @click="showDiagDrawer = false" />
        </v-card-title>
        <v-card-text>
          <!-- Overall status -->
          <div class="mb-4">
            <StatusChip v-if="detail" :status="detail.status" />
          </div>

          <!-- Failed Conditions -->
          <div class="text-subtitle-2 mb-2">Failed Conditions</div>
          <div v-if="failedConditions.length === 0" class="text-body-2 text-secondary mb-4">
            All conditions are healthy.
          </div>
          <v-card
            v-for="c in failedConditions"
            :key="c.condition_type"
            variant="outlined"
            class="mb-2"
            color="error"
          >
            <v-card-text class="py-2 px-3">
              <div class="d-flex align-center mb-1">
                <v-icon icon="mdi-close-circle" color="error" size="small" class="mr-2" />
                <span class="font-weight-medium">{{ c.condition_type }}</span>
                <v-spacer />
                <v-chip size="x-small" variant="tonal" color="error">{{ c.status }}</v-chip>
              </div>
              <div v-if="c.reason" class="text-caption font-weight-medium">{{ c.reason }}</div>
              <div v-if="c.message" class="text-caption text-secondary mt-1">{{ c.message }}</div>
            </v-card-text>
          </v-card>

          <!-- Failing Containers -->
          <div class="text-subtitle-2 mt-4 mb-2">Failing Containers</div>
          <div v-if="failingPodStatuses.length === 0" class="text-body-2 text-secondary mb-4">
            No containers are reporting errors.
          </div>
          <v-card
            v-for="(fs, i) in failingPodStatuses"
            :key="i"
            variant="outlined"
            class="mb-3"
            color="warning"
          >
            <v-card-text class="py-2 px-3">
              <div class="d-flex align-center mb-1">
                <v-icon icon="mdi-cube-outline" size="small" class="mr-2" />
                <span class="text-caption font-weight-medium">{{ fs.podName }}</span>
                <v-chip size="x-small" variant="tonal" class="ml-2">{{ fs.containerName }}</v-chip>
              </div>
              <v-chip size="x-small" color="error" variant="flat" class="mb-2">
                {{ fs.reason }}
              </v-chip>
              <v-alert
                type="info"
                density="compact"
                variant="tonal"
                class="text-caption"
              >
                {{ fs.summary }}
              </v-alert>
            </v-card-text>
          </v-card>
        </v-card-text>
      </v-card>
    </v-navigation-drawer>
  </div>
</template>
