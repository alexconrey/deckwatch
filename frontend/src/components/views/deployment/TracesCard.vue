<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { tracingApi, traceUrlFor, type TraceSummary } from "@/api/tracing";

// Recent traces for a deployment. Polls the backend on a slow cadence
// (10s) because trace ingestion is async and users rarely need sub-second
// freshness — mirrors the resource-metrics card polling story rather than
// the logs viewer's tight loop.
//
// The card auto-hides when tracing is not configured AND no addon is
// attached, so an install without tracing wired up isn't cluttered with
// a permanent "not configured" callout. Once the addon lands the card
// appears with the callout, guiding the operator toward settings.

const props = defineProps<{
  namespace: string;
  deploymentName: string;
  // From DeploymentDetailPage: the attached-addons list, filtered to
  // otel-collector. Empty means no tracing sidecar → we still render the
  // card if the backend returns traces (backend may be receiving spans
  // from a manually-instrumented app) but hide the "attach the addon"
  // hint.
  hasCollectorAddon: boolean;
}>();

const traces = ref<TraceSummary[]>([]);
const uiUrl = ref<string>("");
const backendKind = ref<string>("tempo");
const unavailableReason = ref<string | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
let pollHandle: number | null = null;

const POLL_INTERVAL_MS = 10_000;
const TRACE_LIMIT = 20;

const load = async () => {
  loading.value = true;
  error.value = null;
  try {
    const res = await tracingApi.listTraces(
      props.namespace,
      props.deploymentName,
      // Sidecar defaults service.name to the deployment name via
      // interpolate() in src/handlers/addons.rs -- same value here.
      props.deploymentName,
      TRACE_LIMIT,
    );
    traces.value = res.traces;
    uiUrl.value = res.ui_url;
    backendKind.value = res.backend_kind || "tempo";
    unavailableReason.value = res.unavailable_reason ?? null;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to load traces";
  } finally {
    loading.value = false;
  }
};

onMounted(() => {
  load();
  pollHandle = window.setInterval(load, POLL_INTERVAL_MS);
});

onUnmounted(() => {
  if (pollHandle !== null) {
    window.clearInterval(pollHandle);
    pollHandle = null;
  }
});

watch(
  () => [props.namespace, props.deploymentName],
  () => {
    traces.value = [];
    load();
  },
);

// Card is hidden when: no traces, no backend, no addon attached. That
// keeps deployments without tracing wired up from getting a dead card.
const shouldRender = computed(() => {
  if (props.hasCollectorAddon) return true;
  if (traces.value.length > 0) return true;
  if (unavailableReason.value !== null) return false;
  return false;
});

const linkForTrace = (traceId: string) =>
  traceUrlFor(backendKind.value, uiUrl.value, traceId);

const formatDuration = (ms: number): string => {
  if (ms < 1) return "<1ms";
  if (ms < 1000) return `${ms.toFixed(0)}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
};

const formatTimestamp = (ms: number): string => {
  if (!ms) return "-";
  const d = new Date(ms);
  return d.toLocaleString();
};

// Trace IDs are 16 or 32 hex chars. Truncate for the row display so the
// table stays readable; full ID goes in the tooltip and the link.
const shortId = (id: string): string =>
  id.length > 12 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id;
</script>

<template>
  <v-card v-if="shouldRender" class="mb-4">
    <v-card-title class="d-flex align-center">
      <v-icon icon="mdi-timeline-clock-outline" class="mr-2" size="small" />
      <span class="text-subtitle-1">Recent Traces</span>
      <v-chip
        v-if="traces.length > 0"
        size="x-small"
        variant="tonal"
        class="ml-2"
      >
        {{ traces.length }}
      </v-chip>
      <v-spacer />
      <v-btn
        size="small"
        variant="text"
        icon="mdi-refresh"
        :loading="loading"
        title="Refresh traces"
        @click="load"
      />
    </v-card-title>

    <v-alert
      v-if="error"
      type="error"
      density="compact"
      class="mx-4 mb-2"
      closable
      @click:close="error = null"
    >
      {{ error }}
    </v-alert>

    <v-alert
      v-if="unavailableReason"
      type="info"
      density="compact"
      variant="tonal"
      class="mx-4 mb-2"
    >
      {{ unavailableReason }}
    </v-alert>

    <div
      v-else-if="traces.length === 0 && !loading"
      class="text-center py-4 text-secondary text-body-2"
    >
      No traces recorded yet
      <div v-if="hasCollectorAddon" class="text-caption mt-1">
        The collector is attached — spans should appear within a few seconds
        of the next request.
      </div>
    </div>

    <v-table v-else-if="traces.length > 0" density="compact">
      <thead>
        <tr>
          <th>Trace ID</th>
          <th>Root Span</th>
          <th class="text-right">Duration</th>
          <th class="text-right">Spans</th>
          <th>Time</th>
          <th />
        </tr>
      </thead>
      <tbody>
        <tr v-for="t in traces" :key="t.trace_id">
          <td>
            <v-tooltip location="top" open-delay="300">
              <template #activator="{ props: tipProps }">
                <span v-bind="tipProps" class="text-mono text-caption">
                  {{ shortId(t.trace_id) }}
                </span>
              </template>
              <span>{{ t.trace_id }}</span>
            </v-tooltip>
          </td>
          <td class="text-caption">{{ t.root_span_name }}</td>
          <td class="text-right text-caption">
            {{ formatDuration(t.duration_ms) }}
          </td>
          <td class="text-right text-caption">
            {{ t.span_count || "?" }}
          </td>
          <td class="text-caption text-secondary">
            {{ formatTimestamp(t.timestamp_ms) }}
          </td>
          <td class="text-right">
            <v-btn
              v-if="linkForTrace(t.trace_id)"
              :href="linkForTrace(t.trace_id) ?? undefined"
              target="_blank"
              rel="noopener noreferrer"
              size="x-small"
              variant="text"
              icon="mdi-open-in-new"
              :title="`Open trace in ${backendKind}`"
            />
          </td>
        </tr>
      </tbody>
    </v-table>
  </v-card>
</template>
