import { onMounted, onUnmounted, ref } from "vue";
import { eventsApi } from "@/api/events";
import type { EventSummary } from "@/types/api";
import { useClusterAlertSettings } from "./useClusterAlertSettings";

// Ambient cluster-alert toast queue. Polls the cluster-wide events endpoint
// (Warning only) on a 15s cadence and enqueues a toast for each *new* event
// -- one whose last_timestamp is strictly newer than the highest we have
// seen. Toast state lives at module scope so any component (e.g. the
// `AlertToastStack` mounted in App.vue) sees the same reactive queue.

const POLL_INTERVAL_MS = 15_000;
const TOAST_TTL_MS = 5_000;
const MAX_VISIBLE = 3;

export interface AlertToast {
  id: number;
  reason: string;
  involvedLabel: string;
  message: string;
  namespace: string | null;
  createdAt: number;
}

const toasts = ref<AlertToast[]>([]);
let nextId = 1;

// Highest last_timestamp we have already surfaced. We use a millisecond epoch
// to avoid string-comparing RFC3339 timestamps with mixed timezone offsets.
// Seeded to `Date.now()` on the first poll so the operator does not get a
// flood of stale warnings the moment they open a browser tab.
let lastSeenMs: number | null = null;
let seeded = false;

// Tracks toast IDs -> their auto-dismiss timers so we can cancel when the
// user clicks-to-dismiss or when the toast falls off the tail.
const dismissTimers = new Map<number, ReturnType<typeof setTimeout>>();

// Backend keeps `namespace + name` unique per event object across a cluster,
// so we key already-shown events on that pair. Guards against the same
// warning firing more than once because its `count` bumped inside the
// polling window.
const shownKeys = new Set<string>();

function eventKey(ev: EventSummary): string {
  return `${ev.namespace}/${ev.name}`;
}

function eventTimestampMs(ev: EventSummary): number | null {
  const raw = ev.last_timestamp ?? ev.first_timestamp;
  if (!raw) return null;
  const parsed = Date.parse(raw);
  return Number.isFinite(parsed) ? parsed : null;
}

function involvedLabelOf(ev: EventSummary): string {
  const base = `${ev.involved_object_kind}/${ev.involved_object_name}`;
  if (ev.involved_object_namespace) {
    return `${ev.involved_object_namespace} / ${base}`;
  }
  return base;
}

function scheduleAutoDismiss(id: number) {
  const timer = setTimeout(() => {
    dismissToast(id);
  }, TOAST_TTL_MS);
  dismissTimers.set(id, timer);
}

export function dismissToast(id: number) {
  const timer = dismissTimers.get(id);
  if (timer) {
    clearTimeout(timer);
    dismissTimers.delete(id);
  }
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

function enqueueToast(ev: EventSummary) {
  const toast: AlertToast = {
    id: nextId++,
    reason: ev.reason || ev.event_type || "Warning",
    involvedLabel: involvedLabelOf(ev),
    message: (ev.message ?? "").trim(),
    namespace: ev.involved_object_namespace ?? ev.namespace ?? null,
    createdAt: Date.now(),
  };
  toasts.value = [...toasts.value, toast];
  scheduleAutoDismiss(toast.id);

  // Trim from the head so the newest events stay visible.
  while (toasts.value.length > MAX_VISIBLE) {
    const oldest = toasts.value[0];
    dismissToast(oldest.id);
  }
}

async function poll() {
  try {
    const resp = await eventsApi.listCluster({
      fieldSelector: "type=Warning",
    });

    // First poll just calibrates the watermark -- we do not want to spray
    // toasts for pre-existing warnings the operator has already seen (or
    // long since forgotten about).
    if (!seeded) {
      let maxTs = Date.now();
      for (const ev of resp.events) {
        shownKeys.add(eventKey(ev));
        const ts = eventTimestampMs(ev);
        if (ts !== null && ts > maxTs) maxTs = ts;
      }
      lastSeenMs = maxTs;
      seeded = true;
      return;
    }

    // Sort ascending by timestamp so toasts appear in chronological order.
    const sorted = [...resp.events].sort((a, b) => {
      const ta = eventTimestampMs(a) ?? 0;
      const tb = eventTimestampMs(b) ?? 0;
      return ta - tb;
    });

    let newWatermark = lastSeenMs ?? 0;
    for (const ev of sorted) {
      const ts = eventTimestampMs(ev);
      if (ts === null) continue;
      if (lastSeenMs !== null && ts <= lastSeenMs) continue;

      const key = eventKey(ev);
      if (shownKeys.has(key)) {
        if (ts > newWatermark) newWatermark = ts;
        continue;
      }
      shownKeys.add(key);
      enqueueToast(ev);
      if (ts > newWatermark) newWatermark = ts;
    }
    lastSeenMs = newWatermark;

    // Trim the `shownKeys` set so it does not grow unbounded across a
    // long-running session. 500 is well above `MAX_VISIBLE` and any
    // realistic per-poll delta, so we will not re-notify on churn.
    if (shownKeys.size > 500) {
      const keep = Array.from(shownKeys).slice(-250);
      shownKeys.clear();
      for (const k of keep) shownKeys.add(k);
    }
  } catch {
    // Swallow errors -- an ambient background poll should not surface fetch
    // failures. The rest of the UI will still show its own error state if
    // the API is down.
  }
}

// Ref-counted lifecycle so the composable can be used in more than one place
// without stopping the poll when the first consumer unmounts. Currently
// mounted only in `AlertToastStack`, but the pattern keeps things safe if
// another component ever calls it.
let activeConsumers = 0;
let pollTimer: ReturnType<typeof setInterval> | null = null;

function startPolling() {
  if (pollTimer !== null) return;
  void poll();
  pollTimer = setInterval(() => void poll(), POLL_INTERVAL_MS);
}

function stopPolling() {
  if (pollTimer === null) return;
  clearInterval(pollTimer);
  pollTimer = null;
}

function onVisibilityChange() {
  if (activeConsumers === 0) return;
  if (document.hidden) {
    stopPolling();
  } else {
    startPolling();
  }
}

export function useClusterAlerts() {
  const { enabled } = useClusterAlertSettings();

  onMounted(() => {
    activeConsumers += 1;
    if (activeConsumers === 1) {
      document.addEventListener("visibilitychange", onVisibilityChange);
    }
    startPolling();
  });

  onUnmounted(() => {
    activeConsumers -= 1;
    if (activeConsumers === 0) {
      stopPolling();
      document.removeEventListener("visibilitychange", onVisibilityChange);
      // Clear any pending toast timers so tests / hot-reload do not leak.
      for (const timer of dismissTimers.values()) clearTimeout(timer);
      dismissTimers.clear();
      toasts.value = [];
      // Reset the watermark so a re-mount recalibrates rather than
      // replaying stale events.
      seeded = false;
      lastSeenMs = null;
      shownKeys.clear();
    }
  });

  return {
    toasts,
    dismissToast,
    enabled,
  };
}
