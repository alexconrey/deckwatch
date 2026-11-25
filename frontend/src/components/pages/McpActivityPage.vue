<script setup lang="ts">
// MCP activity: what MCP clients changed or were refused (SEC-22 audit rows),
// and how the tools are used (deckwatch's MCP metrics from Prometheus: its
// OTLP histogram, or its /metrics counters as the fallback).
import { computed, reactive, ref, watch } from "vue";
import { auditApi } from "@/api/audit";
import { prometheusQueryApi } from "@/api/prometheusQuery";
import { settingsApi } from "@/api/settings";
import { traceUrlFor } from "@/api/tracing";
import { usePolling } from "@/composables/usePolling";
import TimeSeriesCard from "@/components/common/TimeSeriesCard.vue";
import type {
  AuditEntry,
  McpActivityResponse,
  McpActivityWindow,
  TracingSettings,
} from "@/types/api";
import { formatTimestamp } from "@/utils/format";
import {
  ACTIONS,
  OUTCOME_VALUES,
  WINDOWS,
  activityTotals,
  auditTarget,
  clientLabel,
  errorRateSeries,
  formatCount,
  formatPercent,
  formatRate,
  formatSeconds,
  formatTick,
  outcomeMeta,
  parseMcpDetail,
  sourceMeta,
  sumSeries,
  windowStart,
} from "@/utils/mcpActivity";

const timeWindow = ref<McpActivityWindow>("24h");

// --- Metrics (Prometheus) ---------------------------------------------------

const activity = ref<McpActivityResponse | null>(null);
const metricsLoading = ref(false);
const metricsError = ref<string | null>(null);

async function loadMetrics() {
  metricsLoading.value = true;
  try {
    activity.value = await prometheusQueryApi.mcpActivity(timeWindow.value);
    metricsError.value = null;
  } catch (e) {
    metricsError.value = e instanceof Error ? e.message : "Failed to load MCP metrics";
  } finally {
    metricsLoading.value = false;
  }
}

const metricsUnavailable = computed(() => activity.value?.unavailable_reason ?? null);
const hasMetrics = computed(() => activity.value !== null && !metricsUnavailable.value);
const source = computed(() => sourceMeta(activity.value?.source ?? null));
const totals = computed(() => activityTotals(activity.value?.tools ?? []));
const callsPerMinute = computed(() => sumSeries(activity.value?.calls_per_minute ?? []));
const errorRate = computed(() => errorRateSeries(activity.value?.calls_per_minute ?? []));
const applyManifest = computed(() => sumSeries(activity.value?.apply_manifest_per_minute ?? []));
const p95 = computed(() => activity.value?.p95_seconds ?? []);
const latestP95 = computed(() => p95.value.at(-1)?.v ?? null);
const multiDay = computed(() => timeWindow.value === "7d");

const perMinute = (v: number) => `${formatRate(v)} /min`;
const percent = (v: number) => formatPercent(v);
const percentTick = (v: number) => `${formatTick(v)}%`;

const toolHeaders = [
  { title: "Tool", key: "tool", sortable: true },
  { title: "Calls", key: "calls", sortable: true, align: "end" as const },
  { title: "Errors", key: "errors", sortable: true, align: "end" as const },
  { title: "Denied", key: "denied", sortable: true, align: "end" as const },
  { title: "p95", key: "p95_seconds", sortable: true, align: "end" as const },
];

// --- Audit rows (changes and denials) ----------------------------------------

const filters = reactive<{ action: string | null; outcome: string | null; client: string | null }>({
  action: null,
  outcome: null,
  client: null,
});
const rows = ref<AuditEntry[]>([]);
const nextCursor = ref<string | null>(null);
const rowsLoading = ref(false);
const rowsError = ref<string | null>(null);
// Every client name seen in the window, for the filter.
const seenClients = ref<string[]>([]);

function auditQuery(cursor?: string) {
  return {
    source: "mcp" as const,
    since: windowStart(timeWindow.value),
    action: filters.action ?? undefined,
    outcome: filters.outcome ?? undefined,
    client: filters.client ?? undefined,
    limit: 50,
    cursor,
  };
}

async function loadRows(more = false) {
  rowsLoading.value = true;
  try {
    const page = await auditApi.list(auditQuery(more ? (nextCursor.value ?? undefined) : undefined));
    rows.value = more ? [...rows.value, ...page.items] : page.items;
    nextCursor.value = page.next_cursor;
    const clients = new Set(seenClients.value);
    for (const r of page.items) if (r.client_name) clients.add(r.client_name);
    seenClients.value = [...clients].sort();
    rowsError.value = null;
  } catch (e) {
    rowsError.value = e instanceof Error ? e.message : "Failed to load the audit log";
  } finally {
    rowsLoading.value = false;
  }
}

const actionItems: string[] = [...ACTIONS];
const outcomeItems: string[] = [...OUTCOME_VALUES];

const deniedCount = computed(() => rows.value.filter((r) => r.outcome === "denied").length);

interface Row {
  entry: AuditEntry;
  client: string | null;
  keys: string[];
  values: string;
  deniedRule: string | null;
  traceUrl: string | null;
}

const tracing = ref<TracingSettings | null>(null);

const tableRows = computed<Row[]>(() =>
  rows.value.map((entry) => {
    const detail = parseMcpDetail(entry);
    const traceId = detail?.trace_id;
    const ui = tracing.value?.ui_url ?? "";
    return {
      entry,
      client: clientLabel(entry, detail),
      keys: detail?.argument_keys ?? [],
      values: Object.entries(detail?.arguments ?? {})
        .map(([k, v]) => `${k}=${v}`)
        .join(" "),
      deniedRule: detail?.denied_rule ?? null,
      // Only when deckwatch exports traces (the row has a trace id) and a
      // tracing UI is configured in Settings.
      traceUrl: traceId && ui ? traceUrlFor(tracing.value?.backend_kind ?? "tempo", ui, traceId) : null,
    };
  }),
);

async function loadTracing() {
  try {
    tracing.value = (await settingsApi.get()).tracing ?? null;
  } catch {
    tracing.value = null;
  }
}

// --- Refresh -----------------------------------------------------------------

let tracingLoaded = false;
async function refresh() {
  if (!tracingLoaded) {
    tracingLoaded = true;
    void loadTracing();
  }
  await Promise.all([loadMetrics(), loadRows()]);
}

const { refresh: refreshNow } = usePolling(refresh, 30000);

watch(timeWindow, () => {
  seenClients.value = [];
  refreshNow();
});
watch(filters, () => void loadRows());
</script>

<template>
  <div>
    <div class="d-flex align-center flex-wrap ga-3 mb-1">
      <h2 class="text-h5">MCP activity</h2>
      <v-spacer />
      <v-btn-toggle
        v-model="timeWindow"
        mandatory
        density="compact"
        variant="outlined"
        divided
        aria-label="Time window"
        data-testid="window-toggle"
      >
        <v-btn v-for="w in WINDOWS" :key="w.value" :value="w.value" size="small">
          {{ w.title }}
        </v-btn>
      </v-btn-toggle>
    </div>
    <p class="text-body-2 text-medium-emphasis mb-4">
      Tool calls from MCP clients: every change and every refused call from the audit log, and how
      the tools are used, from Prometheus.
    </p>

    <!-- Metrics -->
    <v-alert v-if="metricsError" type="error" variant="tonal" class="mb-4">{{ metricsError }}</v-alert>
    <v-alert
      v-else-if="metricsUnavailable"
      type="info"
      variant="tonal"
      class="mb-4"
      data-testid="metrics-unavailable"
    >
      Tool call metrics need Prometheus: {{ metricsUnavailable }}. The audit log below works without
      it.
    </v-alert>

    <template v-if="hasMetrics">
      <div class="d-flex align-center flex-wrap ga-2 mb-3" data-testid="metrics-source">
        <v-chip :color="source.color" :prepend-icon="source.icon" size="small" variant="tonal" label>
          {{ source.label }}
        </v-chip>
        <span class="text-body-2 text-medium-emphasis">{{ source.detail }}</span>
      </div>

      <v-row dense class="mb-2" data-testid="kpi-row">
        <v-col cols="6" md="3">
          <v-card variant="outlined" class="pa-3">
            <div class="text-caption text-medium-emphasis">Tool calls</div>
            <div class="text-h5" data-testid="kpi-calls">{{ formatCount(totals.calls) }}</div>
          </v-card>
        </v-col>
        <v-col cols="6" md="3">
          <v-card variant="outlined" class="pa-3">
            <div class="text-caption text-medium-emphasis">Failed or denied</div>
            <div class="text-h5" data-testid="kpi-error-rate">{{ formatPercent(totals.errorRate) }}</div>
          </v-card>
        </v-col>
        <v-col cols="6" md="3">
          <v-card variant="outlined" class="pa-3">
            <div class="text-caption text-medium-emphasis">Denied</div>
            <div class="text-h5" data-testid="kpi-denied">{{ formatCount(totals.denied) }}</div>
          </v-card>
        </v-col>
        <v-col cols="6" md="3">
          <v-card variant="outlined" class="pa-3">
            <div class="text-caption text-medium-emphasis">p95 duration, latest</div>
            <div class="text-h5" data-testid="kpi-p95">{{ formatSeconds(latestP95) }}</div>
          </v-card>
        </v-col>
      </v-row>

      <v-row dense class="mb-2">
        <v-col cols="12" md="6">
          <TimeSeriesCard
            title="Tool calls per minute"
            :points="callsPerMinute"
            :format="perMinute"
            :axis-format="formatTick"
            :show-day="multiDay"
            :loading="metricsLoading"
          />
        </v-col>
        <v-col cols="12" md="6">
          <TimeSeriesCard
            title="Failed or denied calls"
            :points="errorRate"
            :format="percent"
            :axis-format="percentTick"
            :show-day="multiDay"
            :loading="metricsLoading"
          />
        </v-col>
        <v-col cols="12" md="6">
          <TimeSeriesCard
            title="p95 tool call duration"
            :points="p95"
            :format="formatSeconds"
            :show-day="multiDay"
            :loading="metricsLoading"
          />
        </v-col>
        <v-col cols="12" md="6">
          <TimeSeriesCard
            title="apply_manifest calls per minute"
            :points="applyManifest"
            :format="perMinute"
            :axis-format="formatTick"
            :show-day="multiDay"
            :loading="metricsLoading"
          />
        </v-col>
      </v-row>

      <v-card variant="outlined" class="mb-4">
        <v-card-title class="text-subtitle-1">Calls by tool</v-card-title>
        <v-data-table
          :headers="toolHeaders"
          :items="activity?.tools ?? []"
          density="compact"
          items-per-page="10"
          :sort-by="[{ key: 'calls', order: 'desc' }]"
          data-testid="tool-table"
        >
          <template #item.calls="{ value }"><span class="tabular">{{ formatCount(value) }}</span></template>
          <template #item.errors="{ value }"><span class="tabular">{{ formatCount(value) }}</span></template>
          <template #item.denied="{ value }"><span class="tabular">{{ formatCount(value) }}</span></template>
          <template #item.p95_seconds="{ value }"><span class="tabular">{{ formatSeconds(value) }}</span></template>
          <template #no-data>No tool calls in this window.</template>
        </v-data-table>
      </v-card>
    </template>

    <!-- Audit rows -->
    <v-card variant="outlined">
      <v-card-title class="d-flex align-center flex-wrap ga-2 text-subtitle-1">
        <span>Changes and denials</span>
        <v-chip v-if="deniedCount" color="warning" size="small" variant="tonal" prepend-icon="mdi-cancel">
          {{ deniedCount }} denied
        </v-chip>
      </v-card-title>
      <v-card-subtitle class="pb-2">
        Every MCP tool call that wrote, deleted or ran a command, and every call a guardrail refused.
        Reads that went through aren't recorded.
      </v-card-subtitle>
      <v-card-text>
        <div class="d-flex flex-wrap ga-3 mb-3">
          <v-select
            v-model="filters.action"
            :items="actionItems"
            label="Action"
            density="compact"
            variant="outlined"
            hide-details
            clearable
            style="max-width: 180px"
            data-testid="filter-action"
          />
          <v-select
            v-model="filters.outcome"
            :items="outcomeItems"
            label="Outcome"
            density="compact"
            variant="outlined"
            hide-details
            clearable
            style="max-width: 180px"
            data-testid="filter-outcome"
          />
          <v-select
            v-model="filters.client"
            :items="seenClients"
            label="Client (self-reported)"
            density="compact"
            variant="outlined"
            hide-details
            clearable
            style="max-width: 240px"
            data-testid="filter-client"
          />
        </div>

        <v-alert v-if="rowsError" type="error" variant="tonal" class="mb-3">{{ rowsError }}</v-alert>

        <div
          v-if="!rowsLoading && !rowsError && rows.length === 0"
          class="text-center py-6 text-medium-emphasis text-body-2"
          data-testid="no-audit-rows"
        >
          No MCP changes or denials in this window.
        </div>

        <v-table v-else density="compact" :style="{ opacity: rowsLoading ? 0.6 : 1 }">
          <thead>
            <tr>
              <th>Time</th>
              <th>Tool</th>
              <th>Outcome</th>
              <th>Target</th>
              <th>Arguments</th>
              <th>Client</th>
              <th />
            </tr>
          </thead>
          <tbody>
            <tr v-for="r in tableRows" :key="r.entry.id" data-testid="audit-row">
              <td class="text-no-wrap">{{ formatTimestamp(r.entry.timestamp) }}</td>
              <td>
                <div class="font-weight-medium">{{ r.entry.tool }}</div>
                <div class="text-caption text-medium-emphasis">{{ r.entry.action }}</div>
              </td>
              <td>
                <v-chip
                  :color="outcomeMeta(r.entry.outcome).color"
                  :prepend-icon="outcomeMeta(r.entry.outcome).icon"
                  size="small"
                  variant="tonal"
                  label
                >
                  {{ outcomeMeta(r.entry.outcome).label }}
                </v-chip>
                <div v-if="r.deniedRule" class="text-caption text-medium-emphasis" data-testid="denied-rule">
                  {{ r.deniedRule }}
                </div>
              </td>
              <td>{{ auditTarget(r.entry) }}</td>
              <td>
                <div v-if="r.values" class="text-caption">{{ r.values }}</div>
                <div v-if="r.keys.length" class="text-caption text-medium-emphasis">
                  keys: {{ r.keys.join(", ") }}
                </div>
              </td>
              <td>
                <v-tooltip
                  v-if="r.client"
                  location="top"
                  text="What the client said about itself when it connected. Not verified: a label, not the caller's identity."
                >
                  <template #activator="{ props: tip }">
                    <span v-bind="tip" class="text-body-2" data-testid="client-says">
                      <span class="text-medium-emphasis">client says</span> {{ r.client }}
                    </span>
                  </template>
                </v-tooltip>
                <span v-else class="text-medium-emphasis">–</span>
              </td>
              <td class="text-right">
                <v-btn
                  v-if="r.traceUrl"
                  :href="r.traceUrl"
                  target="_blank"
                  rel="noopener noreferrer"
                  size="small"
                  variant="text"
                  append-icon="mdi-open-in-new"
                  data-testid="view-trace"
                >
                  View trace
                </v-btn>
              </td>
            </tr>
          </tbody>
        </v-table>

        <div v-if="nextCursor" class="text-center mt-3">
          <v-btn variant="text" :loading="rowsLoading" data-testid="load-more" @click="loadRows(true)">
            Load more
          </v-btn>
        </div>
      </v-card-text>
    </v-card>
  </div>
</template>

<style scoped>
.tabular {
  font-variant-numeric: tabular-nums;
}
</style>
