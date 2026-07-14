<script setup lang="ts">
import { computed, onMounted, ref, onUnmounted, watch } from "vue";
import { diagnosticsApi } from "@/api/diagnostics";
import { ApiError } from "@/api/client";
import type {
  AiQuotaSnapshot,
  DiagAgent,
  DiagStatus,
  DiagnosticStatusResponse,
} from "@/types/api";
import { useAiSettings } from "@/composables/useAiSettings";

const props = defineProps<{
  namespace: string;
  podName: string;
  container?: string;
  logs: string;
  podPhase?: string;
}>();

const AGENT_KEY = "deckwatch-ai-agent";
const CRASH_STATES = new Set([
  "CrashLoopBackOff",
  "Error",
  "Failed",
  "ImagePullBackOff",
  "ErrImagePull",
  "CreateContainerError",
]);

const showChooser = ref(false);
const savedAgent = ref<DiagAgent | null>(
  (sessionStorage.getItem(AGENT_KEY) as DiagAgent | null) ?? null,
);

const { claudeEnabled, codexEnabled } = useAiSettings();

const submitting = ref(false);
const submitError = ref<string | null>(null);
const jobName = ref<string | null>(null);
const jobStatus = ref<DiagStatus | null>(null);
const jobDetail = ref<DiagnosticStatusResponse | null>(null);
const jobOutput = ref<string>("");
const streamPhase = ref<string | null>(null);
const streaming = ref(false);
// Rate-limit snapshot for this namespace. Fetched on mount so the chip
// renders immediately with the truth from the server; refreshed after
// every submit (successful or 429) so the operator can see the counter
// tick down or the retry window shrink.
const quota = ref<AiQuotaSnapshot | null>(null);
// 429 error path: keep the message + retry countdown separate from
// submitError so we can render it as a distinct "over quota" affordance
// rather than a generic red banner.
const rateLimitedUntil = ref<number | null>(null); // ms epoch
let source: EventSource | null = null;
let pollHandle: ReturnType<typeof setInterval> | null = null;
// True once the SSE stream has produced at least one useful event. Guards
// the polling fallback: if the browser drops the connection before we ever
// received a `log`/`status`/`done` event we assume SSE isn't going to work
// for this deployment (proxy stripping text/event-stream, bearer-auth
// blocking the unauthenticated EventSource, etc.) and fall back to the
// classic poll+fetch flow.
let sseGotEvent = false;

const errorPattern = /\b(error|fatal)\b/i;

// Hide the whole feature when the operator has turned off every provider.
const anyProviderEnabled = computed(
  () => claudeEnabled.value || codexEnabled.value,
);

const showButton = computed(() => {
  if (!anyProviderEnabled.value) return false;
  if (props.podPhase && CRASH_STATES.has(props.podPhase)) return true;
  if (props.logs && errorPattern.test(props.logs)) return true;
  return false;
});

// If the previously-remembered agent has since been disabled in settings,
// forget it so the chooser reopens on the next click.
watch(
  [savedAgent, claudeEnabled, codexEnabled],
  () => {
    if (savedAgent.value === "claude" && !claudeEnabled.value) {
      savedAgent.value = null;
      sessionStorage.removeItem(AGENT_KEY);
    }
    if (savedAgent.value === "codex" && !codexEnabled.value) {
      savedAgent.value = null;
      sessionStorage.removeItem(AGENT_KEY);
    }
  },
  { immediate: true },
);

const showResultCard = computed(
  () => jobName.value !== null || submitError.value !== null,
);

const isTerminal = computed(
  () => jobStatus.value === "succeeded" || jobStatus.value === "failed",
);

const outputDisplay = computed(() => {
  if (jobOutput.value) return jobOutput.value;
  if (streaming.value) {
    switch (streamPhase.value) {
      case "waiting_for_pod":
        return "(waiting for agent pod to be scheduled...)";
      case "pod_found":
        return "(agent pod scheduled, waiting for it to start...)";
      case "streaming":
        return "(agent is starting, output will appear here...)";
      default:
        return "(connecting to agent output stream...)";
    }
  }
  if (isTerminal.value) return "(no output)";
  return "";
});

// Quota chip cosmetics: neutral when there's plenty of headroom, warning
// as it approaches empty, error once the limiter is going to reject. The
// chip is intentionally always visible when the feature is available so
// operators build a mental model of the cap before hitting it.
const quotaColor = computed(() => {
  const q = quota.value;
  if (!q) return "info";
  if (q.remaining === 0) return "error";
  if (q.remaining <= Math.max(1, Math.floor(q.limit * 0.25))) return "warning";
  return "info";
});

const quotaLabel = computed(() => {
  const q = quota.value;
  if (!q) return "";
  return `${q.remaining}/${q.limit} AI jobs left this hour`;
});

const rateLimitCountdown = ref<string | null>(null);
let countdownHandle: ReturnType<typeof setInterval> | null = null;

function startCountdown(untilMs: number) {
  stopCountdown();
  const tick = () => {
    const remaining = Math.max(0, Math.floor((untilMs - Date.now()) / 1000));
    if (remaining === 0) {
      rateLimitCountdown.value = null;
      rateLimitedUntil.value = null;
      stopCountdown();
      // The window has probably freed up a slot — refresh so the chip
      // updates without waiting for the operator to try again.
      void loadQuota();
      return;
    }
    const mins = Math.floor(remaining / 60);
    const secs = remaining % 60;
    rateLimitCountdown.value =
      mins > 0 ? `${mins}m ${secs}s` : `${secs}s`;
  };
  tick();
  countdownHandle = setInterval(tick, 1000);
}

function stopCountdown() {
  if (countdownHandle) {
    clearInterval(countdownHandle);
    countdownHandle = null;
  }
}

async function loadQuota() {
  try {
    quota.value = await diagnosticsApi.quota(props.namespace);
  } catch {
    // Non-fatal: chip just stays hidden. Don't blast an error dialog for
    // a quota lookup — the operator will still see a real 429 on submit
    // if the limiter engages.
  }
}

function selectAgent(agent: DiagAgent) {
  if (agent === "codex" && !codexEnabled.value) return;
  if (agent === "claude" && !claudeEnabled.value) return;
  savedAgent.value = agent;
  sessionStorage.setItem(AGENT_KEY, agent);
  showChooser.value = false;
  void submitDiagnostic(agent);
}

function changeAgent() {
  savedAgent.value = null;
  sessionStorage.removeItem(AGENT_KEY);
  showChooser.value = true;
}

function onDiagnoseClick() {
  submitError.value = null;
  if (rateLimitedUntil.value && rateLimitedUntil.value > Date.now()) {
    // Guard against double-clicks while over quota — the button is disabled
    // in that state anyway, but a fast keyboard user could beat the render.
    return;
  }
  if (savedAgent.value) {
    void submitDiagnostic(savedAgent.value);
  } else {
    showChooser.value = true;
  }
}

async function submitDiagnostic(agent: DiagAgent) {
  submitting.value = true;
  submitError.value = null;
  jobName.value = null;
  jobStatus.value = null;
  jobDetail.value = null;
  jobOutput.value = "";
  streamPhase.value = null;
  disconnectStream();
  stopPolling();

  try {
    const resp = await diagnosticsApi.create(props.namespace, {
      pod_name: props.podName,
      container: props.container,
      logs: props.logs,
      agent,
    });
    jobName.value = resp.job_name;
    jobStatus.value = resp.status;
    quota.value = resp.quota;
    connectStream(resp.job_name);
  } catch (e) {
    if (e instanceof ApiError && e.status === 429) {
      // Backend enforces the sliding-window cap. It returns the retry
      // window in the JSON body AND in the Retry-After header — we only
      // see the body here (fetch doesn't surface headers unless caller
      // switches to raw fetch), which is fine because the body is
      // authoritative.
      const body = e.body as unknown as {
        retry_after_secs?: number;
        limit?: number;
        used?: number;
        message?: string;
      };
      const retrySecs = body.retry_after_secs ?? 60;
      rateLimitedUntil.value = Date.now() + retrySecs * 1000;
      startCountdown(rateLimitedUntil.value);
      submitError.value =
        body.message ??
        `AI job quota exceeded for this namespace. Try again in ${retrySecs}s.`;
      // Reflect the exhausted state immediately without a round-trip.
      if (body.limit !== undefined && body.used !== undefined) {
        quota.value = {
          limit: body.limit,
          used: body.used,
          remaining: 0,
          reset_in_secs: retrySecs,
        };
      } else {
        void loadQuota();
      }
    } else {
      submitError.value =
        e instanceof ApiError
          ? e.body?.message ?? e.message
          : e instanceof Error
            ? e.message
            : "Failed to start diagnostic";
    }
  } finally {
    submitting.value = false;
  }
}

// SSE-first result flow. When it works we get lines as the agent emits them;
// when it doesn't (auth, proxy, dropped connection before any event fired)
// we quietly fall back to the old poll+fetch behavior so the operator never
// sees a permanently spinning card.
function connectStream(job: string) {
  disconnectStream();
  sseGotEvent = false;
  streaming.value = true;

  // Also refresh the metadata sidebar (started_at / completed_at) in
  // parallel with the SSE stream — the SSE endpoint doesn't emit those.
  startStatusPolling();

  const url = diagnosticsApi.streamUrl(props.namespace, job);
  const es = new EventSource(url);
  source = es;

  es.addEventListener("status", (event: MessageEvent) => {
    sseGotEvent = true;
    try {
      const data = JSON.parse(event.data) as { phase?: string };
      if (data.phase) streamPhase.value = data.phase;
    } catch {
      /* ignore malformed status */
    }
  });

  es.addEventListener("log", (event: MessageEvent) => {
    sseGotEvent = true;
    try {
      const data = JSON.parse(event.data) as { line: string };
      // Preserve the agent's own line breaks — each event is one already-
      // terminated line from kube's log_stream reader.
      jobOutput.value += (jobOutput.value ? "\n" : "") + data.line;
    } catch {
      /* ignore malformed log */
    }
  });

  es.addEventListener("error", (event: MessageEvent) => {
    // Named "error" events come from the server (drive_diag_stream sends them
    // on lookup failures). Distinguish from the transport-level `onerror` by
    // checking for a data payload.
    if (event.data) {
      sseGotEvent = true;
      try {
        const data = JSON.parse(event.data) as { message?: string };
        if (data.message) {
          jobOutput.value +=
            (jobOutput.value ? "\n" : "") + `[stream error] ${data.message}`;
        }
      } catch {
        /* ignore */
      }
    }
  });

  es.addEventListener("done", (event: MessageEvent) => {
    sseGotEvent = true;
    try {
      const data = JSON.parse(event.data) as { status?: DiagStatus };
      if (data.status) jobStatus.value = data.status;
    } catch {
      /* ignore malformed done */
    }
    // A `done` event means the job is terminal — close the stream so the
    // browser doesn't try to reconnect and re-drive the whole state machine.
    finishStream();
  });

  // Transport-level failure. If we never received any event, fall back to
  // polling; otherwise assume the stream ended and let the status poller
  // catch the terminal state on its next tick.
  es.onerror = () => {
    if (!sseGotEvent) {
      disconnectStream();
      streaming.value = false;
      streamPhase.value = null;
      void pollStatusOnce({ fetchOnTerminal: true });
    } else {
      disconnectStream();
      streaming.value = false;
    }
  };
}

function disconnectStream() {
  source?.close();
  source = null;
}

function finishStream() {
  disconnectStream();
  streaming.value = false;
  streamPhase.value = null;
  stopPolling();
  // One last status fetch so completed_at etc. show up even if the poll
  // interval hasn't fired yet.
  void pollStatusOnce({ fetchOnTerminal: false });
}

function startStatusPolling() {
  stopPolling();
  pollHandle = setInterval(() => {
    void pollStatusOnce({ fetchOnTerminal: false });
  }, 3000);
  void pollStatusOnce({ fetchOnTerminal: false });
}

function stopPolling() {
  if (pollHandle) {
    clearInterval(pollHandle);
    pollHandle = null;
  }
}

// Two responsibilities: keep the sidebar metadata fresh while streaming,
// and drive the whole show when SSE isn't available. `fetchOnTerminal`
// controls whether we also GET the /result endpoint on completion — needed
// only for the polling fallback since a working stream already delivered
// stdout line-by-line.
async function pollStatusOnce(opts: { fetchOnTerminal: boolean }) {
  if (!jobName.value) return;
  try {
    const s = await diagnosticsApi.status(props.namespace, jobName.value);
    jobDetail.value = s;
    // Don't overwrite a status we already got from the SSE `done` event;
    // the SSE view is authoritative for the terminal transition.
    if (!isTerminal.value) jobStatus.value = s.status;
    if (s.status === "succeeded" || s.status === "failed") {
      stopPolling();
      if (opts.fetchOnTerminal) {
        await fetchResult();
      }
    }
  } catch (e) {
    // Keep polling on transient errors — job may not be visible yet.
    if (e instanceof ApiError && e.status !== 404) {
      submitError.value = e.body?.message ?? e.message;
      stopPolling();
    }
  }
}

async function fetchResult() {
  if (!jobName.value) return;
  try {
    const r = await diagnosticsApi.result(props.namespace, jobName.value);
    jobOutput.value = r.output || "";
  } catch (e) {
    jobOutput.value =
      e instanceof ApiError
        ? `Failed to fetch output: ${e.body?.message ?? e.message}`
        : `Failed to fetch output: ${e instanceof Error ? e.message : String(e)}`;
  }
}

function dismissResult() {
  disconnectStream();
  stopPolling();
  jobName.value = null;
  jobStatus.value = null;
  jobDetail.value = null;
  jobOutput.value = "";
  streamPhase.value = null;
  streaming.value = false;
  submitError.value = null;
}

const statusColor = computed(() => {
  switch (jobStatus.value) {
    case "succeeded":
      return "success";
    case "failed":
      return "error";
    case "running":
      return "info";
    default:
      return "warning";
  }
});

const agentLabel = (a: DiagAgent | null | undefined) => {
  if (a === "claude") return "Claude";
  if (a === "codex") return "Codex";
  return "";
};

const buttonDisabled = computed(
  () =>
    submitting.value ||
    (quota.value?.remaining === 0) ||
    (rateLimitedUntil.value !== null && rateLimitedUntil.value > Date.now()),
);

const disabledTooltip = computed(() => {
  if (rateLimitedUntil.value && rateLimitCountdown.value) {
    return `AI job quota exhausted — retry in ${rateLimitCountdown.value}`;
  }
  if (quota.value?.remaining === 0) {
    return `AI job quota exhausted for this namespace (${quota.value.used}/${quota.value.limit} used this hour)`;
  }
  return "";
});

// Reset any active job if the pod/container context changes; the namespace
// change also invalidates the quota chip, so refetch when that flips.
watch(
  () => `${props.namespace}/${props.podName}/${props.container ?? ""}`,
  () => {
    dismissResult();
    void loadQuota();
  },
);

onMounted(() => {
  // Only bother loading the quota if the feature is even visible to this
  // user — no need to poke the endpoint from every pod list row.
  if (showButton.value) {
    void loadQuota();
  }
});

// Load the quota lazily when the button becomes visible: some pods only
// transition into a crash state after mount, and we don't want to prefetch
// for every healthy pod on the page.
watch(showButton, (v) => {
  if (v && !quota.value) {
    void loadQuota();
  }
});

onUnmounted(() => {
  disconnectStream();
  stopPolling();
  stopCountdown();
});
</script>

<template>
  <div v-if="showButton" class="diagnose-container">
    <div class="d-flex align-center flex-wrap ga-2">
      <v-tooltip
        :text="disabledTooltip"
        location="bottom"
        :disabled="!buttonDisabled || !disabledTooltip"
      >
        <template #activator="{ props: tipProps }">
          <div v-bind="tipProps" class="d-inline-block">
            <v-btn
              color="primary"
              variant="tonal"
              prepend-icon="mdi-robot"
              :loading="submitting"
              :disabled="buttonDisabled"
              size="small"
              @click="onDiagnoseClick"
            >
              Diagnose with AI
            </v-btn>
          </div>
        </template>
      </v-tooltip>

      <v-chip
        v-if="quota"
        :color="quotaColor"
        size="x-small"
        variant="tonal"
        :prepend-icon="quota.remaining === 0 ? 'mdi-timer-sand' : 'mdi-counter'"
      >
        {{ quotaLabel }}
      </v-chip>

      <v-chip
        v-if="rateLimitCountdown"
        color="error"
        size="x-small"
        variant="flat"
        prepend-icon="mdi-timer-outline"
      >
        Retry in {{ rateLimitCountdown }}
      </v-chip>

      <v-chip
        v-if="savedAgent"
        size="x-small"
        variant="outlined"
        color="secondary"
      >
        Using {{ agentLabel(savedAgent) }}
        <v-btn
          variant="text"
          size="x-small"
          class="ml-1"
          density="compact"
          @click.stop="changeAgent"
        >
          Change
        </v-btn>
      </v-chip>
    </div>

    <!-- Agent selection dialog -->
    <v-dialog v-model="showChooser" max-width="640">
      <v-card>
        <v-card-title class="d-flex align-center">
          <v-icon icon="mdi-robot" class="mr-2" />
          Choose an AI agent
        </v-card-title>
        <v-card-subtitle>
          Your choice is remembered for this browser session.
        </v-card-subtitle>
        <v-card-text>
          <v-row>
            <v-col cols="12" sm="6">
              <v-card
                variant="outlined"
                class="pa-4 agent-card"
                :class="{ 'agent-card-disabled': !claudeEnabled }"
                @click="claudeEnabled && selectAgent('claude')"
              >
                <div class="d-flex align-center mb-2">
                  <v-icon icon="mdi-alpha-c-circle" color="deep-purple" />
                  <span class="text-h6 ml-2">Claude</span>
                  <v-chip
                    v-if="!claudeEnabled"
                    size="x-small"
                    color="warning"
                    variant="tonal"
                    class="ml-2"
                  >
                    Disabled
                  </v-chip>
                </div>
                <div class="text-body-2 text-secondary">
                  Anthropic Claude Code CLI. Good at long-context log
                  reasoning and clear step-by-step diagnoses.
                </div>
              </v-card>
            </v-col>
            <v-col cols="12" sm="6">
              <v-card
                variant="outlined"
                class="pa-4 agent-card agent-card-disabled"
              >
                <div class="d-flex align-center mb-2">
                  <v-icon icon="mdi-alpha-o-circle" color="grey" />
                  <span class="text-h6 ml-2">Codex</span>
                  <v-chip
                    size="x-small"
                    color="info"
                    variant="tonal"
                    class="ml-2"
                  >
                    Coming Soon
                  </v-chip>
                </div>
                <div class="text-body-2 text-secondary">
                  OpenAI Codex CLI. Strong on symbol-level code and
                  concise remediation suggestions.
                </div>
              </v-card>
            </v-col>
          </v-row>
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="showChooser = false">Cancel</v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <!-- Diagnostic status/result panel -->
    <v-card
      v-if="showResultCard"
      variant="outlined"
      class="mt-3 diag-result-card"
    >
      <v-card-title class="d-flex align-center py-2">
        <v-icon icon="mdi-robot" class="mr-2" size="small" />
        <span class="text-body-2">
          AI Diagnostic
          <span v-if="jobDetail?.agent" class="text-caption text-secondary">
            ({{ agentLabel(jobDetail.agent) }})
          </span>
        </span>
        <v-spacer />
        <v-chip
          v-if="streaming"
          size="x-small"
          color="info"
          variant="tonal"
          class="mr-2"
        >
          streaming
        </v-chip>
        <v-chip
          v-if="jobStatus"
          :color="statusColor"
          size="x-small"
          variant="flat"
          class="mr-2"
        >
          {{ jobStatus }}
        </v-chip>
        <v-btn
          icon="mdi-close"
          size="x-small"
          variant="text"
          @click="dismissResult"
        />
      </v-card-title>

      <v-alert v-if="submitError" type="error" density="compact" class="mx-2">
        {{ submitError }}
      </v-alert>

      <v-progress-linear
        v-if="(jobStatus && !isTerminal) || streaming"
        indeterminate
        color="info"
      />

      <div v-if="jobDetail" class="px-3 py-1 text-caption text-secondary">
        Job: <code>{{ jobDetail.job_name }}</code>
        <span v-if="jobDetail.started_at">
          · started {{ jobDetail.started_at }}
        </span>
        <span v-if="jobDetail.completed_at">
          · completed {{ jobDetail.completed_at }}
        </span>
      </div>

      <div v-if="outputDisplay" class="diag-output">{{ outputDisplay }}</div>
    </v-card>
  </div>
</template>

<style scoped>
.diagnose-container {
  margin-top: 8px;
}

.agent-card {
  cursor: pointer;
  transition: border-color 120ms ease;
}
.agent-card:hover {
  border-color: rgb(var(--v-theme-primary));
}
.agent-card-disabled {
  cursor: not-allowed;
  opacity: 0.55;
}
.agent-card-disabled:hover {
  border-color: rgba(var(--v-border-color), var(--v-border-opacity));
}

.diag-result-card {
  overflow: hidden;
}

.diag-output {
  font-family: "JetBrains Mono", "Fira Code", "Consolas", monospace;
  font-size: 12px;
  line-height: 1.5;
  background: #0d1117;
  color: #c9d1d9;
  padding: 12px;
  max-height: 500px;
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-word;
}
</style>
