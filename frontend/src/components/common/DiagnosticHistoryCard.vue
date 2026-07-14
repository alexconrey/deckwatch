<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { diagnosticsApi } from "@/api/diagnostics";
import { ApiError } from "@/api/client";
import type {
  DiagAgent,
  DiagStatus,
  DiagnosticHistoryItem,
  DiagnosticResultResponse,
} from "@/types/api";

const props = withDefaults(
  defineProps<{
    namespace: string;
    // When set, only show entries whose source_pod label matches
    // (the label is a sanitized name segment, so we compare against
    // the sanitized form of the pod name).
    sourcePod?: string;
    // Poll interval in ms. Zero disables polling; useful for tests
    // and for pages that already refresh on their own cadence.
    pollMs?: number;
  }>(),
  { pollMs: 15000 },
);

const items = ref<DiagnosticHistoryItem[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const expanded = ref<string[]>([]);
const resultCache = ref<Record<string, string>>({});
const resultLoading = ref<Record<string, boolean>>({});

let pollHandle: ReturnType<typeof setInterval> | null = null;

// The source-pod label is the same sanitize_name_segment() that the
// backend applies to req.pod_name — mirror that here so filtering
// works for pods with dots/dashes/etc in their names.
function sanitize(input: string): string {
  const cleaned = input
    .split("")
    .map((c) => (/[a-zA-Z0-9]/.test(c) ? c.toLowerCase() : "-"))
    .join("");
  const trimmed = cleaned.replace(/^-+|-+$/g, "");
  const s = trimmed || "pod";
  return s.length > 40 ? s.slice(0, 40) : s;
}

const sourcePodLabel = computed(() =>
  props.sourcePod ? sanitize(props.sourcePod) : null,
);

const filtered = computed(() => {
  if (!sourcePodLabel.value) return items.value;
  return items.value.filter((i) => i.source_pod === sourcePodLabel.value);
});

const headers = [
  { title: "Status", key: "status", sortable: false, width: 110 },
  { title: "Agent", key: "agent", sortable: false, width: 100 },
  { title: "Source pod", key: "source_pod", sortable: false },
  { title: "Started", key: "started_at", sortable: false, width: 200 },
  { title: "Completed", key: "completed_at", sortable: false, width: 200 },
  { title: "", key: "data-table-expand", sortable: false, width: 40 },
];

async function refresh() {
  loading.value = true;
  error.value = null;
  try {
    const resp = await diagnosticsApi.list(props.namespace);
    items.value = resp.items;
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e.body?.message ?? e.message
        : e instanceof Error
          ? e.message
          : "Failed to load diagnostic history";
  } finally {
    loading.value = false;
  }
}

async function loadResult(jobName: string) {
  if (resultCache.value[jobName] !== undefined) return;
  resultLoading.value = { ...resultLoading.value, [jobName]: true };
  try {
    const r: DiagnosticResultResponse = await diagnosticsApi.result(
      props.namespace,
      jobName,
    );
    resultCache.value = {
      ...resultCache.value,
      [jobName]: r.output || "(no output)",
    };
  } catch (e) {
    resultCache.value = {
      ...resultCache.value,
      [jobName]:
        e instanceof ApiError
          ? `Failed to fetch output: ${e.body?.message ?? e.message}`
          : `Failed to fetch output: ${e instanceof Error ? e.message : String(e)}`,
    };
  } finally {
    resultLoading.value = { ...resultLoading.value, [jobName]: false };
  }
}

// Fetch output the moment a row is expanded, so users don't have to
// click twice. Only for terminal statuses — running/pending jobs have
// no useful output yet, and the /result endpoint returns "no pod found"
// until the pod is scheduled.
watch(expanded, (val) => {
  for (const jobName of val) {
    const item = filtered.value.find((i) => i.job_name === jobName);
    if (!item) continue;
    if (item.status === "succeeded" || item.status === "failed") {
      void loadResult(jobName);
    }
  }
});

function statusColor(s: DiagStatus): string {
  switch (s) {
    case "succeeded":
      return "success";
    case "failed":
      return "error";
    case "running":
      return "info";
    default:
      return "warning";
  }
}

function agentLabel(a: DiagAgent | null): string {
  if (a === "claude") return "Claude";
  if (a === "codex") return "Codex";
  return "—";
}

function formatTimestamp(ts: string | null): string {
  if (!ts) return "—";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  return d.toLocaleString();
}

function startPolling() {
  stopPolling();
  if (props.pollMs <= 0) return;
  pollHandle = setInterval(() => {
    void refresh();
  }, props.pollMs);
}

function stopPolling() {
  if (pollHandle) {
    clearInterval(pollHandle);
    pollHandle = null;
  }
}

// Reset + reload when the namespace changes; parent pages remount rarely.
watch(
  () => props.namespace,
  () => {
    items.value = [];
    resultCache.value = {};
    resultLoading.value = {};
    expanded.value = [];
    void refresh();
    startPolling();
  },
);

onMounted(() => {
  void refresh();
  startPolling();
});

onUnmounted(stopPolling);
</script>

<template>
  <v-card variant="outlined" class="diag-history-card">
    <v-card-title class="d-flex align-center py-2">
      <v-icon icon="mdi-history" class="mr-2" size="small" />
      <span class="text-body-1">Diagnostic history</span>
      <v-chip
        v-if="sourcePodLabel"
        size="x-small"
        variant="outlined"
        color="secondary"
        class="ml-2"
      >
        pod: {{ props.sourcePod }}
      </v-chip>
      <v-spacer />
      <v-btn
        icon="mdi-refresh"
        size="x-small"
        variant="text"
        :loading="loading"
        @click="refresh"
      />
    </v-card-title>

    <v-alert v-if="error" type="error" density="compact" class="mx-2 mb-2">
      {{ error }}
    </v-alert>

    <v-data-table
      v-model:expanded="expanded"
      :items="filtered"
      :headers="headers"
      item-value="job_name"
      density="compact"
      class="bg-surface"
      :items-per-page="-1"
      hide-default-footer
      show-expand
      :loading="loading && items.length === 0"
      no-data-text="No diagnostics have been run in this namespace yet."
    >
      <template v-slot:item.status="{ item }">
        <v-chip :color="statusColor(item.status)" size="x-small" variant="flat">
          {{ item.status }}
        </v-chip>
      </template>

      <template v-slot:item.agent="{ item }">
        <span class="text-body-2">{{ agentLabel(item.agent) }}</span>
      </template>

      <template v-slot:item.source_pod="{ item }">
        <span class="text-body-2 font-mono">{{ item.source_pod ?? "—" }}</span>
      </template>

      <template v-slot:item.started_at="{ item }">
        <span class="text-caption text-secondary">
          {{ formatTimestamp(item.started_at) }}
        </span>
      </template>

      <template v-slot:item.completed_at="{ item }">
        <span class="text-caption text-secondary">
          {{ formatTimestamp(item.completed_at) }}
        </span>
      </template>

      <template v-slot:expanded-row="{ item, columns }">
        <tr>
          <td :colspan="columns.length" class="expanded-cell">
            <div class="px-3 py-2 text-caption text-secondary">
              Job: <code>{{ item.job_name }}</code>
              <span v-if="item.created_at">
                · created {{ formatTimestamp(item.created_at) }}
              </span>
            </div>
            <div
              v-if="item.status !== 'succeeded' && item.status !== 'failed'"
              class="px-3 pb-3 text-caption text-secondary"
            >
              Output is available once the job reaches a terminal state.
            </div>
            <div
              v-else-if="resultLoading[item.job_name]"
              class="px-3 pb-3 d-flex align-center"
            >
              <v-progress-circular indeterminate size="16" width="2" />
              <span class="text-caption ml-2">Loading output…</span>
            </div>
            <div
              v-else-if="resultCache[item.job_name] !== undefined"
              class="diag-output"
            >{{ resultCache[item.job_name] }}</div>
          </td>
        </tr>
      </template>
    </v-data-table>
  </v-card>
</template>

<style scoped>
.diag-history-card {
  overflow: hidden;
}

.expanded-cell {
  background: rgba(var(--v-theme-surface-variant), 0.4);
  padding: 0 !important;
}

.diag-output {
  font-family: "JetBrains Mono", "Fira Code", "Consolas", monospace;
  font-size: 12px;
  line-height: 1.5;
  background: #0d1117;
  color: #c9d1d9;
  padding: 12px;
  max-height: 400px;
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-word;
}

.font-mono {
  font-family: "JetBrains Mono", "Fira Code", "Consolas", monospace;
}
</style>
