<script setup lang="ts">
import { computed, ref } from "vue";
import { usePolling } from "@/composables/usePolling";
import { nodesApi } from "@/api/nodes";
import { useNodeMetrics } from "@/composables/useResourceMetrics";
import EventsFeed from "@/components/common/EventsFeed.vue";
import MetricSparkline from "@/components/common/MetricSparkline.vue";
import type { NodeSummary } from "@/types/api";
import { formatAge, formatMemory } from "@/utils/format";

const nodes = ref<NodeSummary[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const expanded = ref<string[]>([]);

const refresh = async () => {
  loading.value = true;
  error.value = null;
  try {
    const response = await nodesApi.list();
    nodes.value = response.nodes;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to fetch nodes";
  } finally {
    loading.value = false;
  }
};

const { refresh: manualRefresh } = usePolling(refresh, 10000);

const {
  series: nodeMetricsSeries,
  latest: nodeMetricsLatest,
  unavailableReason: metricsUnavailable,
} = useNodeMetrics();

const totalNodes = computed(() => nodes.value.length);

const nodeCpuData = (name: string): number[] => {
  const s = nodeMetricsSeries.value.get(name);
  return s ? s.samples.map((x) => x.cpuMillicores) : [];
};
const nodeMemData = (name: string): number[] => {
  const s = nodeMetricsSeries.value.get(name);
  return s ? s.samples.map((x) => x.memBytes) : [];
};
const cpuPercent = (name: string): number | null => {
  const usage = nodeMetricsLatest.value.get(name);
  const node = nodes.value.find((n) => n.name === name);
  if (!usage || !node?.cpu_allocatable) return null;
  const allocMcpu = parseCpuMillicores(node.cpu_allocatable);
  if (allocMcpu === 0) return null;
  return (usage.cpu_millicores / allocMcpu) * 100;
};
const memPercent = (name: string): number | null => {
  const usage = nodeMetricsLatest.value.get(name);
  const node = nodes.value.find((n) => n.name === name);
  if (!usage || !node?.memory_allocatable) return null;
  const allocBytes = parseKibiToBytes(node.memory_allocatable);
  if (allocBytes === 0) return null;
  return (usage.memory_bytes / allocBytes) * 100;
};
const fmtNodeCpu = (name: string): string => {
  const u = nodeMetricsLatest.value.get(name);
  if (!u) return "";
  const pct = cpuPercent(name);
  const base = u.cpu_millicores >= 1000
    ? `${(u.cpu_millicores / 1000).toFixed(2)} cores`
    : `${u.cpu_millicores}m`;
  return pct == null ? base : `${base} (${pct.toFixed(0)}%)`;
};
const fmtNodeMem = (name: string): string => {
  const u = nodeMetricsLatest.value.get(name);
  if (!u) return "";
  const mib = u.memory_bytes / (1024 * 1024);
  const base = mib >= 1024 ? `${(mib / 1024).toFixed(1)} GiB` : `${Math.round(mib)} MiB`;
  const pct = memPercent(name);
  return pct == null ? base : `${base} (${pct.toFixed(0)}%)`;
};

// Local parsers -- node capacity strings from the K8s API arrive as
// `"1500m"` / `"4"` for CPU and `"8123456Ki"` for memory. The metrics
// composable already handles the metrics-server side; these mirror it for
// the capacity side so we can render a %-of-allocatable next to each strip.
function parseCpuMillicores(s: string): number {
  const t = s.trim();
  if (t.endsWith("m")) return Math.round(parseFloat(t.slice(0, -1)) || 0);
  const n = parseFloat(t);
  return Number.isFinite(n) ? Math.round(n * 1000) : 0;
}
function parseKibiToBytes(s: string): number {
  const m = s.trim().match(/^(\d+)(Ki|Mi|Gi|Ti)?$/);
  if (!m) return 0;
  const n = Number(m[1]);
  const unit = m[2] ?? "";
  const factor = unit === "Ti" ? 1024 ** 4
    : unit === "Gi" ? 1024 ** 3
    : unit === "Mi" ? 1024 ** 2
    : unit === "Ki" ? 1024
    : 1;
  return n * factor;
}
const readyNodes = computed(
  () => nodes.value.filter((n) => n.status === "Ready").length,
);
const notReadyNodes = computed(
  () => nodes.value.filter((n) => n.status !== "Ready").length,
);

const headers = [
  { title: "", key: "data-table-expand", width: "48px" },
  { title: "Name", key: "name" },
  { title: "Status", key: "status", width: "120px" },
  { title: "Roles", key: "roles", width: "140px" },
  { title: "CPU", key: "cpu", width: "220px", sortable: false },
  { title: "Memory", key: "memory", width: "260px", sortable: false },
  { title: "Version", key: "kubelet_version", width: "140px" },
  { title: "Age", key: "created_at", width: "100px" },
];


const statusColor = (status: string): string => {
  if (status === "Ready") return "success";
  if (status === "NotReady") return "error";
  return "warning";
};
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <h2 class="text-h5">Cluster Overview</h2>
      <v-spacer />
      <v-btn
        variant="text"
        prepend-icon="mdi-refresh"
        :loading="loading"
        @click="manualRefresh"
      >
        Refresh
      </v-btn>
    </div>

    <v-alert v-if="error" type="error" class="mb-4" closable>
      {{ error }}
    </v-alert>

    <v-row class="mb-4">
      <v-col cols="12" md="4">
        <v-card variant="tonal">
          <v-card-text>
            <div class="text-overline">Total Nodes</div>
            <div class="text-h4">{{ totalNodes }}</div>
          </v-card-text>
        </v-card>
      </v-col>
      <v-col cols="12" md="4">
        <v-card variant="tonal" color="success">
          <v-card-text>
            <div class="text-overline">Ready</div>
            <div class="text-h4">{{ readyNodes }}</div>
          </v-card-text>
        </v-card>
      </v-col>
      <v-col cols="12" md="4">
        <v-card variant="tonal" :color="notReadyNodes > 0 ? 'error' : undefined">
          <v-card-text>
            <div class="text-overline">Not Ready</div>
            <div class="text-h4">{{ notReadyNodes }}</div>
          </v-card-text>
        </v-card>
      </v-col>
    </v-row>

    <v-alert
      v-if="metricsUnavailable"
      type="info"
      density="compact"
      variant="tonal"
      class="mb-3"
    >
      {{ metricsUnavailable }}
    </v-alert>

    <v-data-table
      v-model:expanded="expanded"
      :items="nodes"
      :headers="headers"
      :loading="loading"
      item-value="name"
      show-expand
      class="bg-surface rounded"
    >
      <template v-slot:item.name="{ item }">
        <span class="text-body-1 font-weight-medium">{{ item.name }}</span>
      </template>

      <template v-slot:item.status="{ item }">
        <v-chip :color="statusColor(item.status)" size="small" variant="tonal">
          {{ item.status }}
        </v-chip>
      </template>

      <template v-slot:item.roles="{ item }">
        <span class="text-body-2">{{ item.roles.join(", ") }}</span>
      </template>

      <template v-slot:item.cpu="{ item }">
        <div class="text-body-2 d-flex flex-column ga-1">
          <MetricSparkline
            v-if="!metricsUnavailable"
            :data="nodeCpuData(item.name)"
            :latest-formatted="fmtNodeCpu(item.name)"
            :percent-of-limit="cpuPercent(item.name)"
            :width="90"
            :height="18"
          />
          <div class="text-caption text-secondary">
            alloc {{ item.cpu_allocatable ?? "-" }} / cap {{ item.cpu_capacity ?? "-" }}
          </div>
        </div>
      </template>

      <template v-slot:item.memory="{ item }">
        <div class="text-body-2 d-flex flex-column ga-1">
          <MetricSparkline
            v-if="!metricsUnavailable"
            :data="nodeMemData(item.name)"
            :latest-formatted="fmtNodeMem(item.name)"
            :percent-of-limit="memPercent(item.name)"
            color="secondary"
            :width="90"
            :height="18"
          />
          <div class="text-caption text-secondary">
            alloc {{ formatMemory(item.memory_allocatable) }} / cap {{ formatMemory(item.memory_capacity) }}
          </div>
        </div>
      </template>

      <template v-slot:item.kubelet_version="{ item }">
        <span class="text-body-2">{{ item.kubelet_version ?? "-" }}</span>
      </template>

      <template v-slot:item.created_at="{ item }">
        <span class="text-body-2 text-secondary">
          {{ formatAge(item.created_at) }}
        </span>
      </template>

      <template v-slot:expanded-row="{ columns, item }">
        <tr>
          <td :colspan="columns.length" class="pa-4">
            <div class="mb-2">
              <span class="text-caption text-secondary">OS Image:</span>
              {{ item.os_image ?? "-" }}
            </div>
            <div class="mb-3">
              <span class="text-caption text-secondary">Kernel:</span>
              {{ item.kernel_version ?? "-" }}
            </div>
            <div class="text-subtitle-2 mb-2">Conditions</div>
            <v-table density="compact">
              <thead>
                <tr>
                  <th>Type</th>
                  <th>Status</th>
                  <th>Reason</th>
                  <th>Message</th>
                  <th>Last Transition</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="c in item.conditions" :key="c.condition_type">
                  <td>{{ c.condition_type }}</td>
                  <td>{{ c.status }}</td>
                  <td>{{ c.reason ?? "-" }}</td>
                  <td>{{ c.message ?? "-" }}</td>
                  <td>{{ c.last_transition ?? "-" }}</td>
                </tr>
              </tbody>
            </v-table>
          </td>
        </tr>
      </template>

      <template v-slot:no-data>
        <div class="text-center py-8 text-secondary">
          <v-icon icon="mdi-server-network-off" size="48" class="mb-2" />
          <div>No nodes returned</div>
        </div>
      </template>
    </v-data-table>
      <EventsFeed :namespace="null" title="Cluster Events" class="mt-4" />
  </div>
</template>
