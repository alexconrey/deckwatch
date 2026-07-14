<script setup lang="ts">
import { computed } from "vue";
import {
  prometheusAvailable,
  type MetricTimeRangeValue,
} from "@/composables/useResourceMetrics";

// v-model binds directly to the shared `timeRange` ref exported from
// useResourceMetrics -- callers pass it in via v-model, no local mirroring.
defineProps<{
  modelValue: MetricTimeRangeValue;
}>();

defineEmits<{
  "update:modelValue": [value: MetricTimeRangeValue];
}>();

// 15m/1h are served from the in-memory ring buffer. 1d/1w require the
// backend to have a Prometheus URL configured (PROMETHEUS_URL env or
// settings.metrics.prometheus.url); the composable probes on its first
// long-window fetch and populates `prometheusAvailable` from the response.
// While probing, the buttons stay disabled with a "checking..." tooltip.
interface RangeOption {
  value: MetricTimeRangeValue;
  label: string;
  requiresProm: boolean;
}

const options: RangeOption[] = [
  { value: "15m", label: "15m", requiresProm: false },
  { value: "1h", label: "1h", requiresProm: false },
  { value: "1d", label: "1d", requiresProm: true },
  { value: "1w", label: "1w", requiresProm: true },
];

const disabledFor = (opt: RangeOption) => {
  if (!opt.requiresProm) return false;
  // `null` means we have not probed yet -- disable pessimistically so the
  // button doesn't briefly flash enabled then jump back.
  return prometheusAvailable.value !== true;
};

const tooltipFor = (opt: RangeOption) => {
  if (!opt.requiresProm) return "";
  if (prometheusAvailable.value === null) {
    return "Checking Prometheus availability...";
  }
  if (prometheusAvailable.value === false) {
    return "Requires Prometheus. Set PROMETHEUS_URL on the deckwatch pod.";
  }
  return "";
};

// Purely for template readability -- v-tooltip only wraps the button when
// disabled, so the hover activator survives Vuetify's pointer-events:none.
const anyDisabled = computed(() =>
  options.some((o) => disabledFor(o)),
);
</script>

<template>
  <v-btn-toggle
    :model-value="modelValue"
    mandatory
    density="compact"
    variant="outlined"
    divided
    color="primary"
    @update:model-value="(v: MetricTimeRangeValue) => $emit('update:modelValue', v)"
  >
    <template v-for="opt in options" :key="opt.value">
      <v-tooltip
        v-if="disabledFor(opt)"
        location="top"
        :text="tooltipFor(opt)"
      >
        <template #activator="{ props: tipProps }">
          <!-- v-btn-toggle keys off child v-btn instances so we still render
               the disabled button; the wrapping span carries the tooltip
               activator so the hover target survives even when the button
               itself blocks pointer events. -->
          <span v-bind="tipProps">
            <v-btn
              :value="opt.value"
              size="small"
              disabled
            >
              {{ opt.label }}
            </v-btn>
          </span>
        </template>
      </v-tooltip>
      <v-btn
        v-else
        :value="opt.value"
        size="small"
      >
        {{ opt.label }}
      </v-btn>
    </template>
  </v-btn-toggle>
  <!-- Keeps the linter happy while still exposing the aggregate state to
       template diagnostics (unused vars would otherwise get elided by
       vue-tsc's stricter setup). -->
  <span v-if="false">{{ anyDisabled }}</span>
</template>
