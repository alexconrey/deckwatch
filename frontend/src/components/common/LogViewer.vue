<script setup lang="ts">
import { ref, computed, watch, nextTick, onUnmounted } from "vue";
import DiagnoseButton from "@/components/common/DiagnoseButton.vue";

const props = defineProps<{
  namespace: string;
  podName: string;
  container?: string;
  tailLines?: number;
  podPhase?: string;
}>();

const MAX_LINES = 100_000;
const TRIM_TO = 75_000;
const RECONNECT_DELAY_MS = 3_000;

type LogLevel = "DEBUG" | "INFO" | "WARN" | "ERROR" | "FATAL" | "UNKNOWN";

const LEVEL_ORDER: LogLevel[] = ["DEBUG", "INFO", "WARN", "ERROR", "FATAL"];

const LEVEL_COLORS: Record<LogLevel, string> = {
  DEBUG: "#8b949e",
  INFO: "#58a6ff",
  WARN: "#d29922",
  ERROR: "#f85149",
  FATAL: "#ff6b6b",
  UNKNOWN: "#c9d1d9",
};

interface ParsedLine {
  raw: string;
  timestamp: string | null;
  level: LogLevel;
}

const lines = ref<ParsedLine[]>([]);
const connected = ref(false);
const loading = ref(false);
const reconnecting = ref(false);
const error = ref<string | null>(null);
const logContainer = ref<HTMLElement | null>(null);
const autoScroll = ref(true);
const search = ref("");
const enabledLevels = ref<Set<LogLevel>>(
  new Set<LogLevel>(["DEBUG", "INFO", "WARN", "ERROR", "FATAL", "UNKNOWN"]),
);
let source: EventSource | null = null;
let reconnectTimer: number | null = null;
let lastHistoryLine: string | null = null;
let firstSseReceived = false;

const isPending = computed(() => props.podPhase === "Pending");

const historyUrl = computed(() => {
  const params = new URLSearchParams();
  if (props.tailLines) params.set("tail_lines", String(props.tailLines));
  if (props.container) params.set("container", props.container);
  const qs = params.toString();
  return `/api/namespaces/${props.namespace}/pods/${props.podName}/logs/history${qs ? "?" + qs : ""}`;
});

const streamUrl = computed(() => {
  const params = new URLSearchParams({
    follow: "true",
    tail_lines: "1",
  });
  if (props.container) params.set("container", props.container);
  return `/api/namespaces/${props.namespace}/pods/${props.podName}/logs?${params}`;
});

// Matches common log-level tokens: [LEVEL], LEVEL:, "level":"LEVEL", or LEVEL as a bare word.
const LEVEL_PATTERN =
  /\b(DEBUG|DBG|INFO|INF|WARN(?:ING)?|ERROR|ERR|FATAL|CRIT(?:ICAL)?)\b/i;

// ISO8601-ish timestamp at start of line (e.g. `kubectl logs --timestamps`),
// or a JSON `"time"|"ts"|"timestamp":"..."` field anywhere in the line.
const TS_PATTERN =
  /^(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)/;
const JSON_TS_PATTERN = /"(?:time|ts|timestamp)"\s*:\s*"([^"]+)"/;

function normalizeLevel(raw: string): LogLevel {
  const u = raw.toUpperCase();
  if (u === "DBG") return "DEBUG";
  if (u === "INF") return "INFO";
  if (u === "WARNING" || u === "WARN") return "WARN";
  if (u === "ERR" || u === "ERROR") return "ERROR";
  if (u === "CRIT" || u === "CRITICAL" || u === "FATAL") return "FATAL";
  if (u === "DEBUG" || u === "INFO") return u as LogLevel;
  return "UNKNOWN";
}

function parseLine(raw: string): ParsedLine {
  let timestamp: string | null = null;
  const tsMatch = TS_PATTERN.exec(raw);
  if (tsMatch) {
    timestamp = tsMatch[1];
  } else {
    const jsonTs = JSON_TS_PATTERN.exec(raw);
    if (jsonTs) timestamp = jsonTs[1];
  }

  let level: LogLevel = "UNKNOWN";
  const lm = LEVEL_PATTERN.exec(raw);
  if (lm) level = normalizeLevel(lm[1]);

  return { raw, timestamp, level };
}

const visibleLines = computed(() => {
  const q = search.value.trim().toLowerCase();
  return lines.value.filter((line) => {
    if (!enabledLevels.value.has(line.level)) return false;
    if (q && !line.raw.toLowerCase().includes(q)) return false;
    return true;
  });
});

const logText = computed(() => lines.value.map((l) => l.raw).join("\n"));

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function highlightSearch(text: string): string {
  const q = search.value.trim();
  if (!q) return escapeHtml(text);
  const escaped = escapeHtml(text);
  const escapedQ = escapeHtml(q).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`(${escapedQ})`, "gi");
  return escaped.replace(re, '<mark class="log-match">$1</mark>');
}

async function loadLogs() {
  disconnect();
  lines.value = [];
  lastHistoryLine = null;
  firstSseReceived = false;
  error.value = null;

  // A Pending pod has no running container yet — fetching logs would 400.
  if (isPending.value) return;

  loading.value = true;
  try {
    const res = await fetch(historyUrl.value);
    if (!res.ok) {
      const body = await res.json();
      throw new Error(body.message || "Failed to fetch logs");
    }
    const data = await res.json();
    const parsed: ParsedLine[] = (data.lines as string[]).map(parseLine);
    lines.value = parsed;
    // Save the last history line for dedup against the first SSE event.
    // The stream is opened with tail_lines=1 which replays the newest
    // history line — comparing by raw text avoids dropping real lines
    // that happen to share a timestamp.
    if (parsed.length > 0) {
      lastHistoryLine = parsed[parsed.length - 1].raw;
    }
    firstSseReceived = false;
    await scrollToBottom();
    connectStream();
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to load logs";
  } finally {
    loading.value = false;
  }
}

function connectStream() {
  clearReconnectTimer();
  source = new EventSource(streamUrl.value);
  connected.value = true;
  reconnecting.value = false;

  source.addEventListener("log", (event: MessageEvent) => {
    const data = JSON.parse(event.data) as { line: string };
    const parsed = parseLine(data.line);

    // Dedup: the stream is opened with tail_lines=1 which replays the newest
    // history line. On the first SSE event only, compare against the last
    // history line — if it matches, skip it. All subsequent events are kept
    // unconditionally so no real log lines are ever dropped.
    if (!firstSseReceived) {
      firstSseReceived = true;
      if (lastHistoryLine !== null && data.line === lastHistoryLine) {
        return;
      }
    }

    lines.value.push(parsed);
    if (lines.value.length > MAX_LINES) {
      lines.value = lines.value.slice(-TRIM_TO);
    }
    if (autoScroll.value) void scrollToBottom();
  });

  source.onerror = () => {
    connected.value = false;
    if (source) {
      source.close();
      source = null;
    }
    scheduleReconnect();
  };
}

function scheduleReconnect() {
  if (reconnectTimer !== null) return;
  reconnecting.value = true;
  reconnectTimer = window.setTimeout(() => {
    reconnectTimer = null;
    if (!isPending.value) connectStream();
  }, RECONNECT_DELAY_MS);
}

function clearReconnectTimer() {
  if (reconnectTimer !== null) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  reconnecting.value = false;
}

function disconnect() {
  clearReconnectTimer();
  source?.close();
  source = null;
  connected.value = false;
}

async function scrollToBottom() {
  await nextTick();
  if (logContainer.value) {
    logContainer.value.scrollTop = logContainer.value.scrollHeight;
  }
}

function clear() {
  lines.value = [];
  lastHistoryLine = null;
  firstSseReceived = false;
}

function toggleLevel(level: LogLevel) {
  const next = new Set(enabledLevels.value);
  if (next.has(level)) next.delete(level);
  else next.add(level);
  enabledLevels.value = next;
}

function download() {
  const blob = new Blob([logText.value], { type: "text/plain" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  const ts = new Date().toISOString().replace(/[:.]/g, "-");
  const container = props.container ? `-${props.container}` : "";
  a.download = `${props.podName}${container}-${ts}.log`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

watch(
  () => `${props.namespace}/${props.podName}/${props.container ?? ""}/${props.tailLines ?? "all"}`,
  () => loadLogs(),
  { immediate: true },
);

watch(
  () => props.podPhase,
  (phase, prev) => {
    // When the pod transitions out of Pending, its container is finally running —
    // kick off history+stream now that logs exist.
    if (prev === "Pending" && phase && phase !== "Pending") {
      loadLogs();
    }
  },
);

onUnmounted(disconnect);
</script>

<template>
  <v-card variant="outlined">
    <v-card-title class="d-flex align-center py-2">
      <v-icon icon="mdi-console" class="mr-2" size="small" />
      <span class="text-body-2">{{ podName }}</span>
      <v-spacer />
      <span class="text-caption text-secondary mr-2">
        {{ visibleLines.length.toLocaleString() }} /
        {{ lines.length.toLocaleString() }} lines
      </span>
      <v-chip
        v-if="loading"
        size="x-small"
        variant="flat"
        color="info"
        class="mr-2"
      >
        Loading...
      </v-chip>
      <v-chip
        v-else-if="reconnecting"
        size="x-small"
        variant="flat"
        color="warning"
        class="mr-2"
      >
        Reconnecting...
      </v-chip>
      <v-chip
        v-else-if="isPending"
        size="x-small"
        variant="flat"
        color="info"
        class="mr-2"
      >
        Pending
      </v-chip>
      <v-chip
        v-else
        :color="connected ? 'success' : 'error'"
        size="x-small"
        variant="flat"
        class="mr-2"
      >
        {{ connected ? "Streaming" : "Disconnected" }}
      </v-chip>
      <v-btn
        icon="mdi-download"
        size="x-small"
        variant="text"
        title="Download logs"
        :disabled="lines.length === 0"
        @click="download"
      />
      <v-btn
        icon="mdi-delete"
        size="x-small"
        variant="text"
        title="Clear"
        @click="clear"
      />
    </v-card-title>

    <div class="px-3 py-1 d-flex align-center flex-wrap ga-2">
      <v-text-field
        v-model="search"
        placeholder="Search logs..."
        density="compact"
        variant="outlined"
        hide-details
        clearable
        prepend-inner-icon="mdi-magnify"
        style="max-width: 320px"
      />
      <div class="d-flex align-center ga-1">
        <v-chip
          v-for="level in LEVEL_ORDER"
          :key="level"
          size="small"
          :variant="enabledLevels.has(level) ? 'flat' : 'outlined'"
          :color="enabledLevels.has(level) ? 'primary' : undefined"
          class="log-level-chip"
          :style="{ borderColor: LEVEL_COLORS[level] }"
          @click="toggleLevel(level)"
        >
          {{ level }}
        </v-chip>
      </div>
    </div>

    <v-alert v-if="error" type="error" density="compact" class="mx-2 my-1">
      {{ error }}
    </v-alert>

    <v-alert
      v-if="isPending && !error"
      type="info"
      density="compact"
      class="mx-2 my-1"
    >
      Pod is starting up, logs will appear when ready
    </v-alert>

    <div
      ref="logContainer"
      class="log-output"
      @scroll="
        () => {
          if (!logContainer) return;
          const el = logContainer;
          autoScroll = el.scrollTop + el.clientHeight >= el.scrollHeight - 20;
        }
      "
    >
      <div
        v-for="(line, i) in visibleLines"
        :key="i"
        class="log-line"
        :class="{
          'log-error': line.level === 'ERROR' || line.level === 'FATAL',
          'log-warn': line.level === 'WARN',
        }"
        :style="{ color: LEVEL_COLORS[line.level] }"
      >
        <span v-html="highlightSearch(line.raw)" />
      </div>
      <div
        v-if="visibleLines.length === 0 && !loading && !isPending"
        class="log-empty"
      >
        {{
          lines.length === 0
            ? "No logs available"
            : "No lines match the current filter"
        }}
      </div>
      <div v-if="loading" class="log-empty">Loading log history...</div>
    </div>

    <div class="pa-2">
      <DiagnoseButton
        :namespace="namespace"
        :pod-name="podName"
        :container="container"
        :logs="logText"
        :pod-phase="podPhase"
      />
    </div>
  </v-card>
</template>

<style scoped>
.log-output {
  font-family: "JetBrains Mono", "Fira Code", "Consolas", monospace;
  font-size: 12px;
  line-height: 1.5;
  background: #0d1117;
  color: #c9d1d9;
  padding: 8px 12px;
  max-height: 500px;
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-all;
}

.log-error {
  background: rgba(248, 81, 73, 0.1);
}

.log-warn {
  background: rgba(210, 153, 34, 0.1);
}

.log-line:hover {
  background: rgba(88, 166, 255, 0.05);
}

.log-empty {
  color: #8b949e;
  font-style: italic;
}

.log-level-chip {
  cursor: pointer;
  user-select: none;
}

:deep(.log-match) {
  background: #f1c40f;
  color: #0d1117;
  border-radius: 2px;
  padding: 0 1px;
}
</style>
