import { onMounted, onUnmounted } from "vue";
import { useRouter } from "vue-router";
import { onCLS, onFCP, onINP, onLCP, onTTFB, type Metric } from "web-vitals";

// Endpoint the backend exposes for frontend metric ingestion. Kept in sync
// with the route registered in `src/routes.rs`.
const INGEST_URL = "/api/frontend-metrics";

// How often to flush the in-memory buffer to the backend. Batching keeps
// request volume low; sendBeacon on visibilitychange catches the tail.
const FLUSH_INTERVAL_MS = 30_000;

interface PageView {
  route: string;
  count: number;
}

interface ApiCall {
  path: string;
  method: string;
  status: number;
  duration_ms: number;
}

interface FrontendError {
  kind: string;
  route: string;
}

interface NavigationTiming {
  route: string;
  load_time_ms: number;
}

interface WebVital {
  name: string;
  value: number;
  route: string;
}

interface MetricsBatch {
  session_id: string;
  page_views: PageView[];
  api_calls: ApiCall[];
  errors: FrontendError[];
  navigation_timing?: NavigationTiming;
  web_vitals: WebVital[];
}

// crypto.randomUUID is available in all evergreen browsers; fall back to a
// short random string for older WebViews.
function newSessionId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return Math.random().toString(36).slice(2, 14);
}

// Module-scoped state — one buffer per browser tab. All useMetrics() callers
// share it, so page-view counts across route changes accumulate correctly.
const sessionId = newSessionId();
const pageViews = new Map<string, number>();
const apiCalls: ApiCall[] = [];
const errors: FrontendError[] = [];
const webVitals: WebVital[] = [];
let pendingNav: NavigationTiming | undefined;
let currentRoute = "unknown";
// web-vitals emits some metrics (LCP, CLS, INP) multiple times as they
// evolve. We keep only the latest value per name so the histogram reflects
// the final measurement the user experienced.
const latestVitals = new Map<string, WebVital>();
let vitalsRegistered = false;

function drain(): MetricsBatch | null {
  // Fold the latest-vital map into the outgoing list right before send so
  // we capture whatever the browser has reported since the last flush.
  const vitalsSnapshot = Array.from(latestVitals.values());
  latestVitals.clear();
  // Also drain any that were pushed to webVitals directly (defensive; the
  // handler currently only writes to latestVitals).
  vitalsSnapshot.push(...webVitals.splice(0, webVitals.length));

  if (
    pageViews.size === 0 &&
    apiCalls.length === 0 &&
    errors.length === 0 &&
    !pendingNav &&
    vitalsSnapshot.length === 0
  ) {
    return null;
  }

  const batch: MetricsBatch = {
    session_id: sessionId,
    page_views: Array.from(pageViews, ([route, count]) => ({ route, count })),
    api_calls: apiCalls.splice(0, apiCalls.length),
    errors: errors.splice(0, errors.length),
    navigation_timing: pendingNav,
    web_vitals: vitalsSnapshot,
  };
  pageViews.clear();
  pendingNav = undefined;
  return batch;
}

// Prefer sendBeacon when the page is unloading — fetch() gets cancelled
// during pagehide, sendBeacon is guaranteed to be queued.
function flush(useBeacon = false): void {
  const batch = drain();
  if (!batch) return;

  const body = JSON.stringify(batch);
  if (useBeacon && navigator.sendBeacon) {
    const blob = new Blob([body], { type: "application/json" });
    navigator.sendBeacon(INGEST_URL, blob);
    return;
  }

  // keepalive lets short fetches survive a navigation
  fetch(INGEST_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body,
    keepalive: true,
  }).catch(() => {
    // Metrics are best-effort — drop on the floor if the backend is down.
  });
}

// Called by every web-vitals callback. Stashes the latest value per metric
// name so drain() can pick it up on the next flush.
function recordVital(metric: Metric): void {
  latestVitals.set(metric.name, {
    name: metric.name,
    value: metric.value,
    route: currentRoute,
  });
}

// One-time registration of web-vitals listeners. Guarded so multiple
// useMetrics() callers don't double-register (each registration would
// deliver duplicate samples).
function registerWebVitals(): void {
  if (vitalsRegistered) return;
  vitalsRegistered = true;
  onLCP(recordVital);
  onCLS(recordVital);
  onINP(recordVital);
  onFCP(recordVital);
  onTTFB(recordVital);
}

/**
 * Record a client-side API call. Wire this in wherever apiFetch() completes
 * or errors — one line at the top of the catch/finally covers it.
 */
export function recordApiCall(
  path: string,
  method: string,
  status: number,
  durationMs: number,
): void {
  // Collapse the URL to its route template so labels stay bounded. The
  // patterns here should track the backend Axum routes.
  const normalized = path
    .replace(/\/namespaces\/[^/]+/, "/namespaces/{ns}")
    .replace(/\/deployments\/[^/]+/, "/deployments/{name}")
    .replace(/\/pods\/[^/]+/, "/pods/{pod_name}")
    .replace(/\/ingresses\/[^/]+/, "/ingresses/{name}")
    .replace(/\/cronjobs\/[^/]+/, "/cronjobs/{name}")
    .replace(/\/containers\/[^/]+/, "/containers/{container_name}")
    .replace(/\/addons\/[^/]+/, "/addons/{addon_id}");

  apiCalls.push({
    path: normalized,
    method: method.toUpperCase(),
    status,
    duration_ms: durationMs,
  });
}

/**
 * Record a client-side error. `kind` should be a short bucket like
 * `network`, `api`, `js`, `unhandled_rejection`.
 */
export function recordError(kind: string, route?: string): void {
  errors.push({ kind, route: route ?? currentRoute });
}

/**
 * Composable that hooks page-view tracking, periodic flush, and lifecycle
 * flush into a component (typically the top-level layout). Safe to call
 * from multiple components — the module-level buffer dedups automatically.
 */
export function useMetrics() {
  const router = useRouter();

  onMounted(() => {
    // Capture initial page-load timing via the Navigation Timing API.
    // Available synchronously after the load event fires.
    if (typeof performance !== "undefined" && "getEntriesByType" in performance) {
      const nav = performance.getEntriesByType(
        "navigation",
      )[0] as PerformanceNavigationTiming | undefined;
      if (nav && nav.loadEventEnd > 0) {
        pendingNav = {
          route: window.location.pathname,
          load_time_ms: nav.loadEventEnd - nav.startTime,
        };
      }
    }

    // Register Core Web Vitals collectors once per tab. Deliveries continue
    // to accumulate through route changes; drain() picks the latest value
    // per metric on each flush.
    registerWebVitals();

    // Track each route change as a page view.
    const stopRouter = router.afterEach((to) => {
      currentRoute = String(to.name ?? to.path);
      pageViews.set(currentRoute, (pageViews.get(currentRoute) ?? 0) + 1);
    });

    // Global handlers for uncaught errors. Attaching here (rather than in
    // main.ts) keeps this composable self-contained.
    const onError = () => recordError("js");
    const onRejection = () => recordError("unhandled_rejection");
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);

    // Flush when the tab goes hidden — most tail data lives here.
    const onVisibility = () => {
      if (document.visibilityState === "hidden") flush(true);
    };
    document.addEventListener("visibilitychange", onVisibility);

    const timer = window.setInterval(() => flush(false), FLUSH_INTERVAL_MS);

    onUnmounted(() => {
      window.clearInterval(timer);
      stopRouter();
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
      document.removeEventListener("visibilitychange", onVisibility);
      flush(true);
    });
  });

  return { recordApiCall, recordError };
}
