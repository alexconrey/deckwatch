<script setup lang="ts">
import { computed, ref, watch, onMounted, onBeforeUnmount } from "vue";
import { Line } from "vue-chartjs";
import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Legend,
  Filler,
  TimeScale,
  type ChartData,
  type ChartOptions,
  type TooltipItem,
} from "chart.js";
import type { Sample } from "@/composables/useResourceMetrics";

ChartJS.register(
  CategoryScale,
  LinearScale,
  TimeScale,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Legend,
  Filler,
);

export interface PanelSeries {
  key: string;
  samples: Sample[];
  color?: string;
}

const props = withDefaults(
  defineProps<{
    series: PanelSeries[];
    title?: string;
    unit?: "cpu" | "memory";
    height?: number;
    metric?: "cpu" | "memory";
  }>(),
  {
    title: "",
    unit: "cpu",
    height: 240,
    metric: undefined,
  },
);

const metricField = computed<"cpuMillicores" | "memBytes">(() => {
  const m = props.metric ?? props.unit;
  return m === "memory" ? "memBytes" : "cpuMillicores";
});

const CPU_COLOR = "#1976d2";
const MEM_COLOR = "#8e24aa";
const PALETTE = [
  "#1976d2", "#8e24aa", "#00897b", "#ef6c00", "#c62828",
  "#5e35b1", "#00acc1", "#7cb342", "#d81b60", "#3949ab",
];

const defaultColor = computed(() =>
  metricField.value === "memBytes" ? MEM_COLOR : CPU_COLOR,
);

function seriesColor(idx: number, override?: string): string {
  if (override) return override;
  if (props.series.length <= 1) return defaultColor.value;
  return PALETTE[idx % PALETTE.length];
}

const axisTimestamps = computed<number[]>(() => {
  let longest: Sample[] = [];
  for (const s of props.series) {
    if (s.samples.length > longest.length) longest = s.samples;
  }
  return longest.map((s) => s.t);
});

function formatTime(t: number): string {
  const d = new Date(t);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  return `${hh}:${mm}:${ss}`;
}

function formatCpu(mcpu: number): string {
  if (mcpu >= 1000) return `${(mcpu / 1000).toFixed(2)} cores`;
  return `${Math.round(mcpu)}m`;
}

function formatMem(bytes: number): string {
  const mib = bytes / (1024 * 1024);
  if (mib >= 1024) return `${(mib / 1024).toFixed(2)} GiB`;
  return `${Math.round(mib)} MiB`;
}

function formatValue(v: number): string {
  return metricField.value === "memBytes" ? formatMem(v) : formatCpu(v);
}

const chartData = computed<ChartData<"line">>(() => {
  const labels = axisTimestamps.value.map(formatTime);
  const axisLen = axisTimestamps.value.length;
  const datasets = props.series.map((s, idx) => {
    const color = seriesColor(idx, s.color);
    const values: (number | null)[] = new Array(axisLen).fill(null);
    const offset = axisLen - s.samples.length;
    for (let i = 0; i < s.samples.length; i++) {
      const raw = s.samples[i][metricField.value];
      values[offset + i] = raw;
    }
    return {
      label: s.key,
      data: values,
      borderColor: color,
      backgroundColor: color + "22",
      fill: props.series.length === 1,
      tension: 0.3,
      pointRadius: 0,
      pointHoverRadius: 4,
      borderWidth: 2,
      spanGaps: true,
    };
  });
  return { labels, datasets };
});

const chartOptions = computed<ChartOptions<"line">>(() => ({
  responsive: true,
  maintainAspectRatio: false,
  animation: false,
  interaction: {
    mode: "index",
    intersect: false,
  },
  plugins: {
    legend: {
      display: props.series.length > 1,
      position: "bottom",
      labels: { boxWidth: 12, font: { size: 11 } },
    },
    tooltip: {
      callbacks: {
        label: (ctx: TooltipItem<"line">) => {
          const v = ctx.parsed.y;
          if (v == null) return `${ctx.dataset.label}: -`;
          return `${ctx.dataset.label}: ${formatValue(v)}`;
        },
      },
    },
    title: {
      display: !!props.title,
      text: props.title,
      font: { size: 13, weight: "bold" as const },
      padding: { bottom: 8 },
    },
  },
  scales: {
    x: {
      ticks: {
        maxTicksLimit: 6,
        font: { size: 10 },
        color: "#777",
      },
      grid: { color: "rgba(0,0,0,0.05)" },
    },
    y: {
      beginAtZero: true,
      ticks: {
        font: { size: 10 },
        color: "#777",
        callback: (val: number | string) => {
          const n = typeof val === "number" ? val : parseFloat(val);
          return formatValue(n);
        },
      },
      grid: { color: "rgba(0,0,0,0.05)" },
    },
  },
}));

const hasData = computed(() =>
  props.series.some((s) => s.samples.length > 0),
);

const chartKey = ref(0);
watch(
  () => props.series.length,
  () => { chartKey.value += 1; },
);

const wrapper = ref<HTMLElement | null>(null);
let resizeObserver: ResizeObserver | null = null;
onMounted(() => {
  if (wrapper.value && typeof ResizeObserver !== "undefined") {
    resizeObserver = new ResizeObserver(() => { chartKey.value += 1; });
    resizeObserver.observe(wrapper.value);
  }
});
onBeforeUnmount(() => {
  if (resizeObserver) resizeObserver.disconnect();
});
</script>

<template>
  <div ref="wrapper" class="metric-panel" :style="{ height: height + 'px' }">
    <div
      v-if="!hasData"
      class="d-flex align-center justify-center text-disabled text-body-2"
      style="height: 100%"
    >
      Collecting samples&hellip;
    </div>
    <Line
      v-else
      :key="chartKey"
      :data="chartData"
      :options="chartOptions"
    />
  </div>
</template>

<style scoped>
.metric-panel {
  position: relative;
  width: 100%;
}
</style>
