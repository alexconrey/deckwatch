<script setup lang="ts">
// Settings → Observability → OpenTelemetry export: the OTLP endpoints
// deckwatch exports its own traces, log lines and metrics to. The parent owns
// the values (they are part of the settings document it saves); this
// component loads GET /api/settings/observability for where each signal goes
// now and after a restart, and for the notice about the retired
// tracing.otlp_endpoint.
import { computed, onMounted, ref } from "vue";
import { settingsApi } from "@/api/settings";
import type { ObservabilitySettings, ObservabilityStatus, OtlpSignalStatus } from "@/types/api";
import {
  SIGNAL_LABELS,
  changesOnRestart,
  current,
  dismissRetiredNotice,
  exportSummary,
  overrideHint,
  retiredNoticeDismissed,
  sourceLabel,
  type OtlpField,
  validateOtlpEndpoint,
} from "@/utils/observability";

const model = defineModel<ObservabilitySettings>({ required: true });

const status = ref<ObservabilityStatus | null>(null);
const statusError = ref<string | null>(null);
const dismissed = ref(false);

async function refresh() {
  try {
    status.value = await settingsApi.observability();
    statusError.value = null;
  } catch (e) {
    statusError.value = (e instanceof Error && e.message) || "Failed to load the export status";
  }
}

onMounted(refresh);
defineExpose({ refresh });

const retired = computed(() => {
  const notice = status.value?.retired_otlp_endpoint;
  if (!notice || dismissed.value || retiredNoticeDismissed(notice.value)) return null;
  return notice;
});

function dismissRetired() {
  if (retired.value) dismissRetiredNotice(retired.value.value);
  dismissed.value = true;
}

function rules(field: OtlpField) {
  return [(v: string | null) => validateOtlpEndpoint(field, v ?? "") ?? true];
}

// Clearing a v-text-field yields null; the API expects "" for "not set".
function update(field: OtlpField, value: string | null) {
  model.value = { ...model.value, [field]: value ?? "" };
}

function sourceColor(s: OtlpSignalStatus): string | undefined {
  switch (current(s).source) {
    case "env":
      return "info";
    case "settings":
      return "primary";
    default:
      return undefined;
  }
}
</script>

<template>
  <div data-testid="otlp-export">
    <h3 class="text-h6 mb-2">OpenTelemetry export</h3>
    <p class="text-body-2 text-secondary mb-3">
      deckwatch can export its own traces, log lines and metrics over OTLP/HTTP:
      traces and logs to an OpenTelemetry Collector, and metrics to Prometheus's
      OTLP receiver. This doesn't change how your workloads export theirs.
    </p>

    <v-alert
      v-if="retired"
      type="warning"
      variant="tonal"
      density="compact"
      closable
      class="mb-3"
      data-testid="otlp-retired-notice"
      @click:close="dismissRetired"
    >
      <div class="font-weight-medium mb-1">Enter the OTLP/HTTP endpoint</div>
      The retired <code>tracing.otlp_endpoint</code> is still set
      (<code>{{ retired.value }}</code>), and deckwatch doesn't use it. It was
      an OTLP/gRPC endpoint, usually on port 4317; deckwatch exports OTLP over
      HTTP, usually port 4318. Enter your collector's OTLP/HTTP endpoint below.
      The old value isn't converted.
    </v-alert>

    <v-alert
      v-if="statusError"
      type="warning"
      variant="tonal"
      density="compact"
      class="mb-3"
      data-testid="otlp-status-error"
    >
      {{ statusError }}
    </v-alert>

    <v-row dense>
      <v-col cols="12" md="6">
        <v-text-field
          :model-value="model.otlp_endpoint"
          label="OTLP endpoint (traces and logs)"
          placeholder="http://otel-collector.observability:4318"
          persistent-placeholder
          :hint="
            overrideHint(status, 'otlp_endpoint') ??
            'The collector\'s OTLP/HTTP base URL. deckwatch appends /v1/traces and /v1/logs.'
          "
          persistent-hint
          :rules="rules('otlp_endpoint')"
          clearable
          variant="outlined"
          density="compact"
          data-testid="otlp-endpoint"
          @update:model-value="update('otlp_endpoint', $event)"
        />
      </v-col>
      <v-col cols="12" md="6">
        <v-text-field
          :model-value="model.otlp_metrics_endpoint"
          label="OTLP metrics endpoint"
          placeholder="http://prometheus.monitoring:9090/api/v1/otlp/v1/metrics"
          persistent-placeholder
          :hint="
            overrideHint(status, 'otlp_metrics_endpoint') ??
            'The full URL of Prometheus\'s OTLP receiver, used as is.'
          "
          persistent-hint
          :rules="rules('otlp_metrics_endpoint')"
          clearable
          variant="outlined"
          density="compact"
          data-testid="otlp-metrics-endpoint"
          @update:model-value="update('otlp_metrics_endpoint', $event)"
        />
      </v-col>
    </v-row>

    <v-alert
      type="info"
      variant="tonal"
      density="compact"
      class="my-3"
      data-testid="otlp-restart-note"
    >
      deckwatch reads these endpoints when it starts, so restart it after you
      save a change. An <code>OTEL_EXPORTER_OTLP_*ENDPOINT</code> environment
      variable wins over them for its signal. Exporter headers from the
      environment, usually credentials, are only sent to endpoints set there.
    </v-alert>

    <v-chip
      v-if="status?.restart_required"
      color="warning"
      variant="tonal"
      size="small"
      prepend-icon="mdi-restart-alert"
      class="mb-2"
      data-testid="otlp-restart-required"
    >
      Restart deckwatch to export to the saved endpoints
    </v-chip>

    <v-table v-if="status" density="compact" data-testid="otlp-signals">
      <thead>
        <tr>
          <th>Signal</th>
          <th>Source</th>
          <th>Exported to</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="s in status.signals" :key="s.signal" :data-testid="`otlp-signal-${s.signal}`">
          <td>{{ SIGNAL_LABELS[s.signal] }}</td>
          <td>
            <v-chip size="x-small" variant="tonal" :color="sourceColor(s)">
              {{ sourceLabel(current(s).source) }}
            </v-chip>
            <div v-if="current(s).from" class="text-caption text-medium-emphasis">
              <code>{{ current(s).from }}</code>
            </div>
          </td>
          <td>
            <span :class="{ 'text-medium-emphasis': !current(s).exported }">
              {{ exportSummary(current(s)) }}
            </span>
            <div
              v-if="changesOnRestart(s)"
              class="text-caption text-warning"
              :data-testid="`otlp-after-restart-${s.signal}`"
            >
              After a restart: {{ exportSummary(s.configured) }}
              ({{ sourceLabel(s.configured.source).toLowerCase() }})
            </div>
          </td>
        </tr>
      </tbody>
    </v-table>
  </div>
</template>
