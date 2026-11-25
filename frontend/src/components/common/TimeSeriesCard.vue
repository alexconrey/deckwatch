<script setup lang="ts">
// One metric over time, as a single-series line in a card: the latest value
// as its label, a crosshair tooltip, and a table view of every point.
import { computed, ref } from "vue";
import { useTheme } from "vuetify";
import { Line } from "vue-chartjs";
import {
  Chart as ChartJS,
  CategoryScale,
  Filler,
  LinearScale,
  LineElement,
  PointElement,
  Tooltip,
  type ChartData,
  type ChartOptions,
  type Plugin,
} from "chart.js";
import type { TimePoint } from "@/types/api";

ChartJS.register(CategoryScale, LinearScale, PointElement, LineElement, Tooltip, Filler);

const props = withDefaults(
  defineProps<{
    title: string;
    points: TimePoint[];
    // Formats a value for the label, tooltip and table ("12.5 /min").
    format: (v: number) => string;
    // Formats axis ticks; defaults to `format`. Ticks can drop the unit the
    // title already names.
    axisFormat?: (v: number) => string;
    // Label time ticks with the day too (multi-day windows).
    showDay?: boolean;
    // Hold the current render, dimmed, while new data loads.
    loading?: boolean;
    height?: number;
  }>(),
  { showDay: false, loading: false, height: 180 },
);

const view = ref<"chart" | "table">("chart");
const theme = useTheme();

// Reference palette (series slot 1 and chart chrome), stepped per mode and
// validated against deckwatch's light and dark surfaces.
const colors = computed(() => {
  const dark = theme.current.value.dark;
  return {
    series: dark ? "#3987e5" : "#2a78d6",
    wash: dark ? "rgba(57, 135, 229, 0.10)" : "rgba(42, 120, 214, 0.10)",
    grid: dark ? "#2c2c2a" : "#e1e0d9",
    axis: dark ? "#383835" : "#c3c2b7",
    muted: "#898781",
    surface: theme.current.value.colors.background,
  };
});

function timeLabel(t: number): string {
  const d = new Date(t);
  const time = d.toLocaleTimeString("en", { hour: "2-digit", minute: "2-digit", hour12: false });
  if (!props.showDay) return time;
  return `${d.toLocaleDateString("en", { weekday: "short" })} ${time}`;
}

const latest = computed(() => props.points.at(-1));

const chartData = computed<ChartData<"line">>(() => ({
  labels: props.points.map((p) => timeLabel(p.t)),
  datasets: [
    {
      label: props.title,
      data: props.points.map((p) => p.v),
      borderColor: colors.value.series,
      backgroundColor: colors.value.wash,
      fill: true,
      borderWidth: 2,
      borderJoinStyle: "round",
      borderCapStyle: "round",
      tension: 0,
      pointRadius: 0,
      pointHoverRadius: 4,
      pointHoverBackgroundColor: colors.value.series,
      pointHoverBorderColor: colors.value.surface,
      pointHoverBorderWidth: 2,
    },
  ],
}));

// A vertical hairline at the hovered point, so readers aim at a time rather
// than at the 2px line.
const crosshair: Plugin<"line"> = {
  id: "crosshair",
  afterDatasetsDraw(chart) {
    const active = chart.tooltip?.getActiveElements?.() ?? [];
    if (!active.length) return;
    const x = active[0].element.x;
    const { top, bottom } = chart.chartArea;
    const ctx = chart.ctx;
    ctx.save();
    ctx.beginPath();
    ctx.moveTo(x, top);
    ctx.lineTo(x, bottom);
    ctx.lineWidth = 1;
    ctx.strokeStyle = colors.value.axis;
    ctx.stroke();
    ctx.restore();
  },
};

const chartOptions = computed<ChartOptions<"line">>(() => ({
  responsive: true,
  maintainAspectRatio: false,
  animation: false,
  interaction: { mode: "index", intersect: false },
  plugins: {
    legend: { display: false },
    tooltip: {
      displayColors: false,
      callbacks: {
        // The value leads, the time follows.
        title: (items) => (items.length ? props.format(items[0].parsed.y ?? 0) : ""),
        label: (item) => String(item.label),
      },
    },
  },
  scales: {
    x: {
      grid: { display: false },
      border: { color: colors.value.axis },
      ticks: { color: colors.value.muted, maxTicksLimit: 6, maxRotation: 0 },
    },
    y: {
      beginAtZero: true,
      grid: { color: colors.value.grid, lineWidth: 1 },
      border: { display: false },
      ticks: {
        color: colors.value.muted,
        maxTicksLimit: 4,
        callback: (v) => (props.axisFormat ?? props.format)(Number(v)),
      },
    },
  },
}));
</script>

<template>
  <v-card variant="outlined" class="time-series-card" data-testid="time-series-card">
    <v-card-title class="d-flex align-center ga-2 text-subtitle-1">
      <span>{{ title }}</span>
      <v-spacer />
      <v-btn
        size="small"
        variant="text"
        :prepend-icon="view === 'chart' ? 'mdi-table' : 'mdi-chart-line'"
        :aria-label="view === 'chart' ? `Show ${title} as a table` : `Show ${title} as a chart`"
        data-testid="toggle-view"
        @click="view = view === 'chart' ? 'table' : 'chart'"
      >
        {{ view === "chart" ? "Table" : "Chart" }}
      </v-btn>
    </v-card-title>
    <v-card-text :style="{ opacity: loading ? 0.5 : 1 }">
      <div class="text-h6 mb-1" data-testid="latest-value">
        <template v-if="latest">{{ format(latest.v) }}</template>
        <span v-else class="text-body-2 text-medium-emphasis">No data in this window</span>
      </div>
      <div v-if="view === 'chart'" :style="{ height: `${height}px` }">
        <Line
          v-if="points.length"
          :data="chartData"
          :options="chartOptions"
          :plugins="[crosshair]"
          :aria-label="`${title} over time`"
          role="img"
        />
      </div>
      <v-table v-else density="compact" fixed-header :height="height" data-testid="series-table">
        <thead>
          <tr>
            <th>Time</th>
            <th class="text-right">{{ title }}</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="p in [...points].reverse()" :key="p.t">
            <td>{{ new Date(p.t).toLocaleString() }}</td>
            <td class="text-right tabular">{{ format(p.v) }}</td>
          </tr>
        </tbody>
      </v-table>
    </v-card-text>
  </v-card>
</template>

<style scoped>
.tabular {
  font-variant-numeric: tabular-nums;
}
</style>
