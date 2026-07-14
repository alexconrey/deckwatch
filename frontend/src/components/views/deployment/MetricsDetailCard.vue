<script setup lang="ts">
import { computed } from "vue";
import type { Series } from "@/composables/useResourceMetrics";
import type { PodUsage } from "@/api/resourceMetrics";
import MetricPanel, { type PanelSeries } from "@/components/common/MetricPanel.vue";

// Renders the 2x2 detail grid seeded from the ring buffer maintained by
// usePodMetrics. All four panels share the same underlying data -- top row
// aggregates across pods, bottom row breaks it out per-pod.
const props = defineProps<{
  series: Map<string, Series>;
  latest: Map<string, PodUsage>;
  loading?: boolean;
}>();

const emit = defineEmits<{
  close: [];
}>();

const podSeriesList = computed(() =>
  Array.from(props.series.values()).sort((a, b) => a.key.localeCompare(b.key)),
);

// Sum across pods per polling tick. Ticks align because usePodMetrics writes
// all pods on each poll -- align by tail so short-lived pods do not skew the
// leading edge of the aggregate.
function aggregate(field: "cpuMillicores" | "memBytes"): PanelSeries {
  const buffers = podSeriesList.value.map((s) => s.samples);
  if (buffers.length === 0) return { key: "Total", samples: [] };
  const maxLen = Math.max(...buffers.map((b) => b.length));
  const samples: { t: number; cpuMillicores: number; memBytes: number }[] = [];
  for (let i = 0; i < maxLen; i++) {
    let sum = 0;
    let t = 0;
    for (const buf of buffers) {
      const s = buf[buf.length - maxLen + i];
      if (s) {
        sum += s[field];
        if (s.t > t) t = s.t;
      }
    }
    const sample = { t, cpuMillicores: 0, memBytes: 0 };
    sample[field] = sum;
    samples.push(sample);
  }
  return { key: "Total", samples };
}

const totalCpu = computed<PanelSeries[]>(() => [aggregate("cpuMillicores")]);
const totalMem = computed<PanelSeries[]>(() => [aggregate("memBytes")]);

const perPodSeries = computed<PanelSeries[]>(() =>
  podSeriesList.value.map((s) => ({
    key: s.key,
    samples: s.samples,
  })),
);

const hasAnyData = computed(() => props.series.size > 0);
</script>

<template>
  <v-card class="mb-4">
    <v-card-title class="d-flex align-center">
      <v-icon icon="mdi-chart-line" start />
      <span class="text-subtitle-1">Resource Metrics</span>
      <v-chip
        v-if="hasAnyData"
        size="x-small"
        variant="tonal"
        color="secondary"
        class="ml-2"
      >
        {{ podSeriesList.length }} pod{{ podSeriesList.length === 1 ? "" : "s" }}
      </v-chip>
      <v-progress-circular
        v-if="loading"
        indeterminate
        size="16"
        width="2"
        color="secondary"
        class="ml-2"
      />
      <v-spacer />
      <v-btn
        variant="text"
        size="small"
        prepend-icon="mdi-chevron-up"
        @click="emit('close')"
      >
        Hide
      </v-btn>
    </v-card-title>

    <v-divider />

    <v-card-text v-if="!hasAnyData" class="text-center py-6 text-secondary">
      Waiting for the first metrics-server sample&hellip;
    </v-card-text>

    <v-container v-else fluid class="pa-2">
      <v-row dense>
        <v-col cols="12" md="6">
          <v-card variant="tonal" class="pa-3">
            <MetricPanel
              :series="totalCpu"
              title="CPU usage (aggregate)"
              unit="cpu"
              :height="220"
            />
          </v-card>
        </v-col>
        <v-col cols="12" md="6">
          <v-card variant="tonal" class="pa-3">
            <MetricPanel
              :series="totalMem"
              title="Memory usage (aggregate)"
              unit="memory"
              :height="220"
            />
          </v-card>
        </v-col>
        <v-col cols="12" md="6">
          <v-card variant="tonal" class="pa-3">
            <MetricPanel
              :series="perPodSeries"
              title="CPU per pod"
              unit="cpu"
              :height="220"
            />
          </v-card>
        </v-col>
        <v-col cols="12" md="6">
          <v-card variant="tonal" class="pa-3">
            <MetricPanel
              :series="perPodSeries"
              title="Memory per pod"
              unit="memory"
              :height="220"
            />
          </v-card>
        </v-col>
      </v-row>
      <div class="text-caption text-secondary mt-2 px-2">
        History is accumulated in-browser from metrics-server polls -- resets on
        page reload. See docs/METRICS_VISUALIZATION.md for the design.
      </div>
    </v-container>
  </v-card>
</template>
