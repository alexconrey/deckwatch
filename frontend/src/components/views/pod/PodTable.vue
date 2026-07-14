<script setup lang="ts">
import { ref, computed } from "vue";
import { useRouter } from "vue-router";
import type { ContainerStatusSummary, PodSummary } from "@/types/api";
import type { Series } from "@/composables/useResourceMetrics";
import type { PodUsage } from "@/api/resourceMetrics";
import PodStatusIcon from "./PodStatusIcon.vue";
import LogViewer from "@/components/common/LogViewer.vue";
import MetricSparkline from "@/components/common/MetricSparkline.vue";

const props = defineProps<{
  pods: PodSummary[];
  namespace: string;
  // Optional per-pod ring buffers keyed by pod name. When omitted, no
  // CPU/mem columns are rendered -- keeps this component reusable in
  // contexts without metrics-server.
  metricsSeries?: Map<string, Series>;
  latestUsage?: Map<string, PodUsage>;
}>();

const router = useRouter();
const emit = defineEmits<{
  "open-portforward": [podName: string]; "open-terminal": [payload: { podName: string; containers: ContainerStatusSummary[] }];
}>();

const expandedLogPods = ref<Set<string>>(new Set());
const expanded = ref<string[]>([]);

const toggleLogs = (podName: string) => {
  const next = new Set(expandedLogPods.value);
  if (next.has(podName)) {
    next.delete(podName);
  } else {
    next.add(podName);
    // Ensure the row is also expanded so the expanded-row slot renders
    if (!expanded.value.includes(podName)) {
      expanded.value = [...expanded.value, podName];
    }
  }
  expandedLogPods.value = next;
};

const openPod = (podName: string) => {
  router.push({
    name: "PodDetail",
    params: { namespace: props.namespace, podName },
  });
};

const hasMetrics = computed(() => !!props.metricsSeries);

const headers = computed(() => {
  const cols: Array<Record<string, unknown>> = [
    { title: "", key: "data-table-expand", width: "40px", sortable: false },
    { title: "", key: "status_icon", width: "40px", sortable: false },
    { title: "Name", key: "name" },
    { title: "Phase", key: "phase", width: "100px" },
    { title: "Probes", key: "conditions", width: "200px", sortable: false },
    { title: "Restarts", key: "restart_count", width: "90px" },
    { title: "Node", key: "node" },
  ];
  if (hasMetrics.value) {
    cols.push({ title: "CPU", key: "cpu_metric", width: "170px", sortable: false });
    cols.push({ title: "Memory", key: "mem_metric", width: "170px", sortable: false });
    cols.push({ title: "Restart trend", key: "restart_metric", width: "140px", sortable: false });
  }
  cols.push({ title: "Fwd", key: "portforward", width: "80px", sortable: false },
  { title: "Exec", key: "exec", width: "80px", sortable: false },
  { title: "Logs", key: "logs", width: "80px", sortable: false });
  return cols;
});

const conditionLabels: Record<string, string> = {
  Initialized: "Init",
  PodScheduled: "Sched",
  ContainersReady: "Ctr",
  Ready: "Ready",
};

const conditionOrder = ["PodScheduled", "Initialized", "ContainersReady", "Ready"];

const containerHeaders = [
  { title: "Name", key: "name" },
  { title: "Image", key: "image" },
  { title: "State", key: "state", width: "110px" },
  { title: "Reason", key: "state_reason" },
  { title: "Ready", key: "ready", width: "80px" },
  { title: "Restarts", key: "restart_count", width: "90px" },
];

const cpuData = (podName: string): number[] => {
  const s = props.metricsSeries?.get(podName);
  return s ? s.samples.map((x) => x.cpuMillicores) : [];
};

const memData = (podName: string): number[] => {
  const s = props.metricsSeries?.get(podName);
  return s ? s.samples.map((x) => x.memBytes) : [];
};

// Restart trend: extract the restart-count series from the ring buffer.
// Skip samples where the backend could not resolve the pod (RBAC etc.) —
// a gap in the sparkline is more honest than plotting a zero.
const restartData = (podName: string): number[] => {
  const s = props.metricsSeries?.get(podName);
  if (!s) return [];
  return s.samples
    .map((x) => x.restartCount)
    .filter((v): v is number => v !== undefined);
};

const restartLatest = (podName: string): string => {
  const data = restartData(podName);
  if (data.length === 0) return "";
  return String(data[data.length - 1]);
};

const fmtCpu = (podName: string): string => {
  const u = props.latestUsage?.get(podName);
  if (!u) return "";
  return `${u.total_cpu_millicores}m`;
};

const fmtMem = (podName: string): string => {
  const u = props.latestUsage?.get(podName);
  if (!u) return "";
  const mib = u.total_memory_bytes / (1024 * 1024);
  if (mib >= 1024) return `${(mib / 1024).toFixed(1)} GiB`;
  return `${Math.round(mib)} MiB`;
};

const stateColor = (state: string): string => {
  switch (state) {
    case "running":
      return "success";
    case "waiting":
      return "warning";
    case "terminated":
      return "error";
    default:
      return "secondary";
  }
};
</script>

<template>
  <v-data-table
    v-model:expanded="expanded"
    :items="pods"
    :headers="headers"
    item-value="name"
    density="compact"
    class="bg-surface"
    :items-per-page="-1"
    hide-default-footer
    show-expand
  >
    <template v-slot:item.status_icon="{ item }">
      <PodStatusIcon :phase="item.phase" :ready="item.ready" />
    </template>

    <template v-slot:item.name="{ item }">
      <div class="d-flex align-center ga-2">
        <a
          href="#"
          class="text-body-2 font-weight-medium text-primary"
          style="cursor: pointer; text-decoration: none;"
          @click.prevent="openPod(item.name)"
        >
          {{ item.name }}
        </a>
        <v-chip
          v-if="item.container_statuses.length > 1"
          size="x-small"
          variant="tonal"
          color="info"
        >
          {{ item.container_statuses.length }} containers
        </v-chip>
        <v-tooltip v-if="item.oom_killed" location="top">
          <template v-slot:activator="{ props: tp }">
            <v-chip
              v-bind="tp"
              size="x-small"
              color="error"
              variant="flat"
              prepend-icon="mdi-memory"
            >
              OOMKilled
            </v-chip>
          </template>
          <span>
            One or more containers hit their memory limit. Raise the memory
            limit or reduce workload memory footprint.
          </span>
        </v-tooltip>
      </div>
      <div v-if="item.container_statuses.length > 0" class="text-caption text-secondary">
        <span
          v-for="cs in item.container_statuses.filter(c => c.state_reason)"
          :key="cs.name"
          class="text-warning"
        >
          {{ cs.state_reason }}
        </span>
      </div>
    </template>

    <template v-slot:item.conditions="{ item }">
      <div class="d-flex ga-1 align-center">
        <v-tooltip
          v-for="type in conditionOrder"
          :key="type"
          location="top"
        >
          <template v-slot:activator="{ props: tp }">
            <v-chip
              v-bind="tp"
              size="x-small"
              :color="item.conditions.find((c: any) => c.condition_type === type)?.status ? 'success' : 'error'"
              variant="flat"
              class="px-1"
              style="min-width: 0"
            >
              <v-icon
                :icon="item.conditions.find((c: any) => c.condition_type === type)?.status ? 'mdi-check' : 'mdi-close'"
                size="x-small"
              />
              <span class="ml-1 text-caption">{{ conditionLabels[type] ?? type }}</span>
            </v-chip>
          </template>
          <div>
            <div class="font-weight-bold">{{ type }}</div>
            <div v-if="item.conditions.find((c: any) => c.condition_type === type)?.reason">
              {{ item.conditions.find((c: any) => c.condition_type === type)?.reason }}
            </div>
            <div v-if="item.conditions.find((c: any) => c.condition_type === type)?.message" class="text-caption">
              {{ item.conditions.find((c: any) => c.condition_type === type)?.message }}
            </div>
          </div>
        </v-tooltip>
      </div>
    </template>

    <template v-slot:item.restart_count="{ item }">
      <v-chip
        v-if="item.restart_count > 0"
        color="warning"
        size="x-small"
        variant="flat"
      >
        {{ item.restart_count }}
      </v-chip>
      <span v-else class="text-secondary">0</span>
    </template>

    <template v-slot:item.node="{ item }">
      <span class="text-body-2 text-secondary">{{ item.node ?? "-" }}</span>
    </template>

    <template v-slot:item.cpu_metric="{ item }">
      <MetricSparkline
        :data="cpuData(item.name)"
        :latest-formatted="fmtCpu(item.name)"
        color="primary"
        :width="70"
        :height="18"
      />
    </template>

    <template v-slot:item.mem_metric="{ item }">
      <MetricSparkline
        :data="memData(item.name)"
        :latest-formatted="fmtMem(item.name)"
        color="secondary"
        :width="70"
        :height="18"
      />
    </template>

    <template v-slot:item.restart_metric="{ item }">
      <MetricSparkline
        :data="restartData(item.name)"
        :latest-formatted="restartLatest(item.name)"
        :color="item.restart_count > 0 ? 'warning' : 'success'"
        :width="70"
        :height="18"
      />
    </template>

    <template v-slot:item.portforward="{ item }">
      <v-btn
        icon="mdi-transit-connection-variant"
        size="x-small"
        variant="text"
        :disabled="item.phase !== 'Running'"
        @click="emit('open-portforward', item.name)"
      />
    </template>

    <template v-slot:item.exec="{ item }">
      <v-btn
        icon="mdi-console"
        size="x-small"
        variant="text"
        :disabled="item.phase !== 'Running'"
        @click="emit('open-terminal', { podName: item.name, containers: item.container_statuses })"
      />
    </template>

    <template v-slot:item.logs="{ item }">
      <v-btn
        icon="mdi-console"
        size="x-small"
        variant="text"
        :color="expandedLogPods.has(item.name) ? 'primary' : undefined"
        @click="toggleLogs(item.name)"
      />
    </template>

    <template v-slot:expanded-row="{ columns, item }">
      <tr>
        <td :colspan="columns.length" class="pa-3 bg-surface-light">
          <div class="text-caption text-secondary mb-2">
            Containers ({{ item.container_statuses.length }})
          </div>
          <v-data-table
            :items="item.container_statuses"
            :headers="containerHeaders"
            item-value="name"
            density="compact"
            class="bg-surface"
            :items-per-page="-1"
            hide-default-footer
          >
            <template v-slot:item.image="{ item: cs }">
              <span class="text-body-2 font-mono">{{ cs.image }}</span>
            </template>

            <template v-slot:item.state="{ item: cs }">
              <v-chip
                size="x-small"
                :color="stateColor(cs.state)"
                variant="flat"
              >
                {{ cs.state }}
              </v-chip>
            </template>

            <template v-slot:item.state_reason="{ item: cs }">
              <div class="d-flex align-center ga-1">
                <span v-if="cs.state_reason" class="text-warning text-caption">
                  {{ cs.state_reason }}
                </span>
                <span v-else class="text-secondary">-</span>
                <v-chip
                  v-if="cs.oom_killed && cs.state_reason !== 'OOMKilled'"
                  size="x-small"
                  color="error"
                  variant="tonal"
                  prepend-icon="mdi-memory"
                >
                  OOM (prev)
                </v-chip>
              </div>
            </template>

            <template v-slot:item.ready="{ item: cs }">
              <v-icon
                :icon="cs.ready ? 'mdi-check-circle' : 'mdi-close-circle'"
                :color="cs.ready ? 'success' : 'error'"
                size="small"
              />
            </template>

            <template v-slot:item.restart_count="{ item: cs }">
              <v-chip
                v-if="cs.restart_count > 0"
                color="warning"
                size="x-small"
                variant="flat"
              >
                {{ cs.restart_count }}
              </v-chip>
              <span v-else class="text-secondary">0</span>
            </template>
          </v-data-table>

          <div v-if="expandedLogPods.has(item.name)" class="mt-3">
            <LogViewer
              :namespace="namespace"
              :pod-name="item.name"
              :pod-phase="item.phase"
            />
          </div>
        </td>
      </tr>
    </template>
  </v-data-table>
</template>
