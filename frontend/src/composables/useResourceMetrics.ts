import { ref, computed, watch, onUnmounted, type Ref } from "vue";
import {
  resourceMetricsApi,
  type PodUsage,
  type NodeUsage,
} from "@/api/resourceMetrics";
import {
  prometheusQueryApi,
  pollIntervalForWindow,
  type PromRangeWindow,
} from "@/api/prometheusQuery";

// Ring-buffer sample. Timestamps are ms since epoch; sparklines only care
// about ordering.
export interface Sample {
  t: number;
  cpuMillicores: number;
  memBytes: number;
}

export interface Series {
  key: string;
  samples: Sample[];
}

// 360 samples * 10 s = ~1 hour of on-page history. See METRICS_VISUALIZATION.md
// section 3 (tier 2) -- this bound is UX-visible telemetry, not a source of
// truth. Ring-buffer is capped so leaving the tab open overnight does not
// leak memory. Callers can narrow the visible window via `timeRange`.
const MAX_SAMPLES = 360;
const POLL_INTERVAL_MS = 10_000;

// Selectable windows for the metrics view. 15m/1h are served from the
// in-memory ring buffer at POLL_INTERVAL_MS. 1d/1w are backed by
// Prometheus range queries -- enabled only when the backend reports a
// configured Prometheus URL, otherwise disabled with a tooltip in
// MetricTimeRange.vue.
export type MetricTimeRangeValue = "15m" | "1h" | "1d" | "1w";

const SAMPLES_PER_RANGE: Record<MetricTimeRangeValue, number> = {
  "15m": 90,
  "1h": 360,
  "1d": 360,
  "1w": 360,
};

// Shared across all composable instances so a single time-range control on
// the page toggles every sparkline together. `timeRange` is exported so
// pages can bind a selector to it directly.
export const timeRange = ref<MetricTimeRangeValue>("15m");

// Reactive flag driven by the first successful Prometheus probe (or the
// first `unavailable_reason` we get back). MetricTimeRange.vue reads this
// to gate the 1d/1w buttons. Starts null (unknown) so the UI can render
// the "checking..." state instead of flashing "unavailable".
export const prometheusAvailable = ref<boolean | null>(null);

// Windows that require Prometheus. Kept as a type guard so a future new
// window ("30d"?) is a compile error rather than a runtime skip.
export function isPrometheusWindow(r: MetricTimeRangeValue): r is PromRangeWindow {
  return r === "1d" || r === "1w";
}

function pushSample(series: Map<string, Series>, key: string, sample: Sample) {
  const buf = series.get(key) ?? { key, samples: [] };
  buf.samples.push(sample);
  if (buf.samples.length > MAX_SAMPLES) {
    buf.samples.splice(0, buf.samples.length - MAX_SAMPLES);
  }
  series.set(key, buf);
}

function evictMissing(series: Map<string, Series>, present: Set<string>) {
  for (const key of Array.from(series.keys())) {
    if (!present.has(key)) series.delete(key);
  }
}

// Return a new Map whose Series only carry the tail `count` samples. Keeps
// the raw ring buffer intact so widening the window later re-hydrates
// without a fresh poll.
function windowed(
  raw: Map<string, Series>,
  count: number,
): Map<string, Series> {
  const out = new Map<string, Series>();
  for (const [key, buf] of raw) {
    const tail = buf.samples.length > count
      ? buf.samples.slice(buf.samples.length - count)
      : buf.samples.slice();
    out.set(key, { key, samples: tail });
  }
  return out;
}

// Merge a fresh Prometheus range response into the ring-buffer Map shape
// that `<MetricPanel>` already knows how to render. One pod = one Series.
// CPU and memory come from separate range queries so we zip them by
// timestamp; if a pod appears in only one we still render what we have.
function promSeriesToMap(
  cpu: { pod: string; points: { t: number; v: number }[] }[],
  mem: { pod: string; points: { t: number; v: number }[] }[],
): Map<string, Series> {
  // Index memory points by pod for O(1) zip.
  const memByPod = new Map<string, Map<number, number>>();
  for (const s of mem) {
    const idx = new Map<number, number>();
    for (const p of s.points) idx.set(p.t, p.v);
    memByPod.set(s.pod, idx);
  }

  const out = new Map<string, Series>();
  const seen = new Set<string>();

  for (const s of cpu) {
    seen.add(s.pod);
    const memIdx = memByPod.get(s.pod);
    const samples: Sample[] = s.points.map((p) => ({
      t: p.t,
      // Backend returns CPU in cores; downstream code speaks millicores.
      cpuMillicores: Math.round(p.v * 1000),
      memBytes: memIdx?.get(p.t) ?? 0,
    }));
    out.set(s.pod, { key: s.pod, samples });
  }

  // Any pod that has memory but no CPU still gets a series.
  for (const s of mem) {
    if (seen.has(s.pod)) continue;
    const samples: Sample[] = s.points.map((p) => ({
      t: p.t,
      cpuMillicores: 0,
      memBytes: p.v,
    }));
    out.set(s.pod, { key: s.pod, samples });
  }

  return out;
}

/**
 * Poll metrics-server for pod usage every 10s and accumulate a per-pod ring
 * buffer suitable for sparklines. `labelSelector` may be reactive; changes
 * reset accumulated state (a different selector is a different query).
 *
 * When `timeRange` is a Prometheus window (1d/1w) AND a deployment name is
 * known, the composable stops polling metrics-server and instead pulls a
 * range from the backend's Prometheus proxy. `series` reads the same
 * `Map<key, Series>` shape either way so `<MetricPanel>` needs no branch.
 *
 * `deployment` is optional -- when unset (e.g. namespace-wide pod list)
 * the Prometheus path silently falls through to the ring buffer, because
 * the query catalog is per-deployment.
 */
export function usePodMetrics(
  namespace: Ref<string>,
  labelSelector?: Ref<string | undefined>,
  deployment?: Ref<string | undefined>,
) {
  const rawSeries = ref<Map<string, Series>>(new Map());
  const promSeries = ref<Map<string, Series>>(new Map());
  const latest = ref<Map<string, PodUsage>>(new Map());
  const unavailableReason = ref<string | null>(null);
  const promUnavailableReason = ref<string | null>(null);
  const promWarnings = ref<string[]>([]);
  const error = ref<string | null>(null);
  const loading = ref(false);

  let timer: ReturnType<typeof setInterval> | null = null;

  const fetchOnce = async () => {
    loading.value = true;
    try {
      const resp = await resourceMetricsApi.listPods(
        namespace.value,
        labelSelector?.value || undefined,
      );
      unavailableReason.value = resp.unavailable_reason ?? null;
      error.value = null;
      const now = Date.now();
      const present = new Set<string>();
      const latestMap = new Map<string, PodUsage>();
      for (const p of resp.pods) {
        present.add(p.name);
        latestMap.set(p.name, p);
        pushSample(rawSeries.value, p.name, {
          t: now,
          cpuMillicores: p.total_cpu_millicores,
          memBytes: p.total_memory_bytes,
        });
      }
      evictMissing(rawSeries.value, present);
      latest.value = latestMap;
    } catch (e) {
      error.value = e instanceof Error ? e.message : "Failed to fetch pod metrics";
    } finally {
      loading.value = false;
    }
  };

  const fetchProm = async () => {
    const dep = deployment?.value;
    if (!dep) {
      // No deployment scope -- can't hit the per-deployment query catalog.
      // Leave prom state as-is; the UI will fall through to the ring buffer.
      return;
    }
    if (!isPrometheusWindow(timeRange.value)) return;
    loading.value = true;
    try {
      const window = timeRange.value;
      const [cpu, mem] = await Promise.all([
        prometheusQueryApi.queryWindow("cpu_usage", namespace.value, dep, window),
        prometheusQueryApi.queryWindow("memory_usage", namespace.value, dep, window),
      ]);
      const anyUnavail = cpu.unavailable_reason ?? mem.unavailable_reason ?? null;
      promUnavailableReason.value = anyUnavail;
      prometheusAvailable.value = anyUnavail === null;
      promWarnings.value = [...(cpu.warnings ?? []), ...(mem.warnings ?? [])];
      promSeries.value = promSeriesToMap(cpu.series, mem.series);
      error.value = null;
    } catch (e) {
      error.value = e instanceof Error ? e.message : "Failed to fetch prometheus range";
      prometheusAvailable.value = false;
    } finally {
      loading.value = false;
    }
  };

  const start = () => {
    stop();
    if (isPrometheusWindow(timeRange.value) && deployment?.value) {
      void fetchProm();
      const interval = pollIntervalForWindow(timeRange.value as PromRangeWindow);
      timer = setInterval(() => void fetchProm(), interval);
    } else {
      void fetchOnce();
      timer = setInterval(() => void fetchOnce(), POLL_INTERVAL_MS);
    }
  };

  const stop = () => {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  };

  // Restart on inputs changing. A different namespace/selector is a different
  // series -- reset accumulated buffers so old pods do not linger. Switching
  // the timeRange between ring-buffer and Prometheus windows also restarts
  // the poller because the source and cadence differ.
  watch(
    () => [
      namespace.value,
      labelSelector?.value ?? "",
      deployment?.value ?? "",
      timeRange.value,
    ] as const,
    () => {
      rawSeries.value = new Map();
      promSeries.value = new Map();
      latest.value = new Map();
      start();
    },
    { immediate: true },
  );

  onUnmounted(stop);

  const series = computed(() => {
    if (isPrometheusWindow(timeRange.value) && deployment?.value) {
      // Prometheus responses are already the right window; no tail slicing.
      return promSeries.value;
    }
    return windowed(rawSeries.value, SAMPLES_PER_RANGE[timeRange.value]);
  });

  return {
    series,
    latest,
    unavailableReason,
    promUnavailableReason,
    promWarnings,
    error,
    loading,
    refresh: () =>
      isPrometheusWindow(timeRange.value) && deployment?.value
        ? fetchProm()
        : fetchOnce(),
    timeRange,
  };
}

/**
 * Poll metrics-server for node usage every 10s. Same ring-buffer treatment
 * as pods -- one series per node keyed by node name.
 *
 * Node-level Prometheus queries are Phase 2 in PROMETHEUS_INTEGRATION.md
 * (needs `node_cpu_seconds_total` / `node_memory_*` from node-exporter, not
 * cAdvisor) and are not wired here yet -- the ring buffer is the sole
 * source for the cluster overview page.
 */
export function useNodeMetrics() {
  const rawSeries = ref<Map<string, Series>>(new Map());
  const latest = ref<Map<string, NodeUsage>>(new Map());
  const unavailableReason = ref<string | null>(null);
  const error = ref<string | null>(null);
  const loading = ref(false);

  let timer: ReturnType<typeof setInterval> | null = null;

  const fetchOnce = async () => {
    loading.value = true;
    try {
      const resp = await resourceMetricsApi.listNodes();
      unavailableReason.value = resp.unavailable_reason ?? null;
      error.value = null;
      const now = Date.now();
      const present = new Set<string>();
      const latestMap = new Map<string, NodeUsage>();
      for (const n of resp.nodes) {
        present.add(n.name);
        latestMap.set(n.name, n);
        pushSample(rawSeries.value, n.name, {
          t: now,
          cpuMillicores: n.cpu_millicores,
          memBytes: n.memory_bytes,
        });
      }
      evictMissing(rawSeries.value, present);
      latest.value = latestMap;
    } catch (e) {
      error.value = e instanceof Error ? e.message : "Failed to fetch node metrics";
    } finally {
      loading.value = false;
    }
  };

  const start = () => {
    stop();
    void fetchOnce();
    timer = setInterval(() => void fetchOnce(), POLL_INTERVAL_MS);
  };

  const stop = () => {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  };

  start();
  onUnmounted(stop);

  const series = computed(() =>
    windowed(rawSeries.value, SAMPLES_PER_RANGE[timeRange.value]),
  );

  return {
    series,
    latest,
    unavailableReason,
    error,
    loading,
    refresh: fetchOnce,
    timeRange,
  };
}
