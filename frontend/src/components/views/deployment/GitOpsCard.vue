<script setup lang="ts">
import { ref, computed, onUnmounted } from "vue";
import { gitopsApi } from "@/api/gitops";
import { diagnosticsApi } from "@/api/diagnostics";
import { ApiError } from "@/api/client";
import { usePolling } from "@/composables/usePolling";
import type {
  GitOpsStatus,
  GitOpsConfigRequest,
  BuildSummary,
  DiagAgent,
  DiagStatus,
  DiagnosticStatusResponse,
} from "@/types/api";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import GitOpsConfigDialog from "./GitOpsConfigDialog.vue";
import LogViewer from "@/components/common/LogViewer.vue";
import AiFixButton from "@/components/common/AiFixButton.vue";
import { formatAge } from "@/utils/format";

const props = defineProps<{
  namespace: string;
  deploymentName: string;
}>();

const status = ref<GitOpsStatus | null>(null);
const builds = ref<BuildSummary[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const showConfigDialog = ref(false);
const showDisableDialog = ref(false);
const showBuilds = ref(false);
const showLogsDialog = ref(false);
const logsJobName = ref<string | null>(null);
const logsPodName = ref<string | null>(null);
const logsPodPhase = ref<string | null>(null);
const logsError = ref<string | null>(null);
const logsLoading = ref(false);

// AI fix state. Only one diagnostic runs from this card at a time —
// fixTargetJob names the build whose logs we handed the agent so the
// operator can tell rows apart in the history table.
const fixTargetJob = ref<string | null>(null);
const fixSubmitting = ref(false);
const fixSubmitError = ref<string | null>(null);
const fixJobName = ref<string | null>(null);
const fixJobStatus = ref<DiagStatus | null>(null);
const fixJobDetail = ref<DiagnosticStatusResponse | null>(null);
const fixJobOutput = ref<string>("");
const fixAgent = ref<DiagAgent | null>(null);
const fixStreamPhase = ref<string | null>(null);
const fixStreaming = ref(false);
let fixPollHandle: ReturnType<typeof setInterval> | null = null;
let fixSource: EventSource | null = null;
// Same guard as DiagnoseButton: fall back to polling only if the SSE stream
// dropped before any event landed. Once we've received data the stream is
// authoritative and its natural termination is `done`.
let fixSseGotEvent = false;

const viewBuildLogs = async (jobName: string) => {
  logsJobName.value = jobName;
  logsPodName.value = null;
  logsPodPhase.value = null;
  logsError.value = null;
  logsLoading.value = true;
  showLogsDialog.value = true;
  try {
    const res = await gitopsApi.listJobPods(props.namespace, jobName);
    const pod = res.pods[0];
    if (!pod) {
      logsError.value = "No pod found for this build job (it may have been cleaned up).";
      return;
    }
    logsPodName.value = pod.name;
    logsPodPhase.value = pod.phase;
  } catch (e) {
    logsError.value = e instanceof Error ? e.message : "Failed to load build pod";
  } finally {
    logsLoading.value = false;
  }
};

const closeLogsDialog = () => {
  showLogsDialog.value = false;
  logsJobName.value = null;
  logsPodName.value = null;
  logsPodPhase.value = null;
  logsError.value = null;
};

const fetchStatus = async () => {
  try {
    status.value = await gitopsApi.getConfig(
      props.namespace,
      props.deploymentName,
    );
  } catch (e) {
    error.value =
      e instanceof Error ? e.message : "Failed to fetch gitops status";
  }
};

const fetchBuilds = async () => {
  try {
    const res = await gitopsApi.listBuilds(
      props.namespace,
      props.deploymentName,
    );
    builds.value = res.builds;
  } catch {
    /* ignore */
  }
};

usePolling(fetchStatus, 5000);

const handleSaveConfig = async (config: GitOpsConfigRequest) => {
  loading.value = true;
  error.value = null;
  try {
    status.value = await gitopsApi.setConfig(
      props.namespace,
      props.deploymentName,
      config,
    );
    showConfigDialog.value = false;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to save config";
  } finally {
    loading.value = false;
  }
};

const handleDisable = async () => {
  loading.value = true;
  try {
    await gitopsApi.deleteConfig(props.namespace, props.deploymentName);
    showDisableDialog.value = false;
    status.value = { enabled: false, config: null, last_commit_sha: null, last_build_status: null, last_build_job: null, last_build_time: null, last_build_error: null };
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to disable";
  } finally {
    loading.value = false;
  }
};

const handleTrigger = async () => {
  loading.value = true;
  error.value = null;
  try {
    await gitopsApi.triggerBuild(props.namespace, props.deploymentName);
    await fetchStatus();
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to trigger build";
  } finally {
    loading.value = false;
  }
};

const toggleBuilds = async () => {
  showBuilds.value = !showBuilds.value;
  if (showBuilds.value) await fetchBuilds();
};

// The diagnose endpoint takes a single `logs` string. Prepend structured
// context so the agent recognizes this as a build failure (not runtime) and
// has enough breadcrumbs (repo, branch, commit, Dockerfile) to investigate.
function buildDiagnosticContext(
  jobName: string,
  commitSha: string | null,
  buildLogs: string,
): string {
  const cfg = status.value?.config;
  const lines = [
    "=== GitOps Build Failure ===",
    `Deployment: ${props.namespace}/${props.deploymentName}`,
    `Build Job: ${jobName}`,
  ];
  if (cfg) {
    lines.push(`Repo URL: ${cfg.repo_url}`);
    lines.push(`Branch: ${cfg.branch}`);
    if (cfg.dockerfile_path) lines.push(`Dockerfile: ${cfg.dockerfile_path}`);
    if (cfg.docker_context) lines.push(`Docker Context: ${cfg.docker_context}`);
  }
  if (commitSha) lines.push(`Commit SHA: ${commitSha}`);
  if (status.value?.last_build_error) {
    lines.push(`Reported Error: ${status.value.last_build_error}`);
  }
  lines.push("", "=== Build Logs ===", buildLogs || "(no logs available)");
  return lines.join("\n");
}

async function onFixBuild(
  jobName: string,
  commitSha: string | null,
  agent: DiagAgent,
) {
  fixTargetJob.value = jobName;
  fixAgent.value = agent;
  fixSubmitting.value = true;
  fixSubmitError.value = null;
  fixJobName.value = null;
  fixJobStatus.value = null;
  fixJobDetail.value = null;
  fixJobOutput.value = "";
  fixStreamPhase.value = null;
  disconnectFixStream();
  stopFixPolling();

  try {
    const podRes = await gitopsApi.listJobPods(props.namespace, jobName);
    const pod = podRes.pods[0];
    if (!pod) {
      throw new Error(
        "Build pod is no longer available - cannot fetch logs for AI fix.",
      );
    }

    let logsText = "";
    try {
      const logsRes = await gitopsApi.getPodLogsHistory(
        props.namespace,
        pod.name,
        { tailLines: 2000 },
      );
      logsText = logsRes.lines.join("\n");
    } catch (e) {
      logsText = `(failed to fetch logs: ${
        e instanceof Error ? e.message : String(e)
      })`;
    }

    const context = buildDiagnosticContext(jobName, commitSha, logsText);

    const resp = await diagnosticsApi.create(props.namespace, {
      pod_name: pod.name,
      logs: context,
      agent,
    });
    fixJobName.value = resp.job_name;
    fixJobStatus.value = resp.status;
    connectFixStream(resp.job_name);
  } catch (e) {
    fixSubmitError.value =
      e instanceof ApiError
        ? e.body?.message ?? e.message
        : e instanceof Error
          ? e.message
          : "Failed to start AI fix";
  } finally {
    fixSubmitting.value = false;
  }
}

// SSE-first result flow, same shape as DiagnoseButton's connectStream: get
// lines as they land, fall back to polling if the browser drops the stream
// before any event arrives. Metadata sidebar (started_at/completed_at) is
// still driven by a parallel status poll — the SSE stream doesn't carry it.
function connectFixStream(job: string) {
  disconnectFixStream();
  fixSseGotEvent = false;
  fixStreaming.value = true;

  startFixStatusPolling();

  const url = diagnosticsApi.streamUrl(props.namespace, job);
  const es = new EventSource(url);
  fixSource = es;

  es.addEventListener("status", (event: MessageEvent) => {
    fixSseGotEvent = true;
    try {
      const data = JSON.parse(event.data) as { phase?: string };
      if (data.phase) fixStreamPhase.value = data.phase;
    } catch {
      /* ignore */
    }
  });

  es.addEventListener("log", (event: MessageEvent) => {
    fixSseGotEvent = true;
    try {
      const data = JSON.parse(event.data) as { line: string };
      fixJobOutput.value +=
        (fixJobOutput.value ? "\n" : "") + data.line;
    } catch {
      /* ignore */
    }
  });

  es.addEventListener("error", (event: MessageEvent) => {
    // Server-emitted named error (has payload). Transport failures land in
    // onerror below with no data.
    if (event.data) {
      fixSseGotEvent = true;
      try {
        const data = JSON.parse(event.data) as { message?: string };
        if (data.message) {
          fixJobOutput.value +=
            (fixJobOutput.value ? "\n" : "") + `[stream error] ${data.message}`;
        }
      } catch {
        /* ignore */
      }
    }
  });

  es.addEventListener("done", (event: MessageEvent) => {
    fixSseGotEvent = true;
    try {
      const data = JSON.parse(event.data) as { status?: DiagStatus };
      if (data.status) fixJobStatus.value = data.status;
    } catch {
      /* ignore */
    }
    finishFixStream();
  });

  es.onerror = () => {
    if (!fixSseGotEvent) {
      disconnectFixStream();
      fixStreaming.value = false;
      fixStreamPhase.value = null;
      void pollFixStatusOnce({ fetchOnTerminal: true });
    } else {
      disconnectFixStream();
      fixStreaming.value = false;
    }
  };
}

function disconnectFixStream() {
  fixSource?.close();
  fixSource = null;
}

function finishFixStream() {
  disconnectFixStream();
  fixStreaming.value = false;
  fixStreamPhase.value = null;
  stopFixPolling();
  void pollFixStatusOnce({ fetchOnTerminal: false });
}

function startFixStatusPolling() {
  stopFixPolling();
  fixPollHandle = setInterval(() => {
    void pollFixStatusOnce({ fetchOnTerminal: false });
  }, 3000);
  void pollFixStatusOnce({ fetchOnTerminal: false });
}

function stopFixPolling() {
  if (fixPollHandle) {
    clearInterval(fixPollHandle);
    fixPollHandle = null;
  }
}

async function pollFixStatusOnce(opts: { fetchOnTerminal: boolean }) {
  if (!fixJobName.value) return;
  try {
    const s = await diagnosticsApi.status(props.namespace, fixJobName.value);
    fixJobDetail.value = s;
    if (!fixTerminal.value) fixJobStatus.value = s.status;
    if (s.status === "succeeded" || s.status === "failed") {
      stopFixPolling();
      if (opts.fetchOnTerminal) {
        await fetchFixResult();
      }
    }
  } catch (e) {
    if (e instanceof ApiError && e.status !== 404) {
      fixSubmitError.value = e.body?.message ?? e.message;
      stopFixPolling();
    }
  }
}

async function fetchFixResult() {
  if (!fixJobName.value) return;
  try {
    const r = await diagnosticsApi.result(props.namespace, fixJobName.value);
    fixJobOutput.value = r.output || "";
  } catch (e) {
    fixJobOutput.value =
      e instanceof ApiError
        ? `Failed to fetch output: ${e.body?.message ?? e.message}`
        : `Failed to fetch output: ${e instanceof Error ? e.message : String(e)}`;
  }
}

function dismissFixResult() {
  disconnectFixStream();
  stopFixPolling();
  fixTargetJob.value = null;
  fixJobName.value = null;
  fixJobStatus.value = null;
  fixJobDetail.value = null;
  fixJobOutput.value = "";
  fixSubmitError.value = null;
  fixAgent.value = null;
  fixStreamPhase.value = null;
  fixStreaming.value = false;
}

const fixOutputDisplay = computed(() => {
  if (fixJobOutput.value) return fixJobOutput.value;
  if (fixStreaming.value) {
    switch (fixStreamPhase.value) {
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
  if (fixTerminal.value) return "(no output)";
  return "";
});

const showFixCard = computed(
  () =>
    fixSubmitting.value ||
    fixJobName.value !== null ||
    fixSubmitError.value !== null,
);

const fixTerminal = computed(
  () => fixJobStatus.value === "succeeded" || fixJobStatus.value === "failed",
);

const fixStatusColor = computed(() => {
  switch (fixJobStatus.value) {
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

onUnmounted(() => {
  disconnectFixStream();
  stopFixPolling();
});

const statusColor = (s: string | null) => {
  switch (s) {
    case "success": return "success";
    case "failed": return "error";
    case "building": return "info";
    default: return "secondary";
  }
};

const statusIcon = (s: string | null) => {
  switch (s) {
    case "success": return "mdi-check-circle";
    case "failed": return "mdi-close-circle";
    case "building": return "mdi-progress-clock";
    default: return "mdi-circle-outline";
  }
};

// Top-level status uses "success"/"failed"; history rows use
// "succeeded"/"failed". Only "failed" is shared, so normalize here.
const isFailedBuildStatus = (s: string | null | undefined): boolean =>
  s === "failed";
</script>

<template>
  <v-card class="mb-4">
    <v-card-title class="d-flex align-center">
      <v-icon icon="mdi-git" class="mr-2" size="small" />
      <span class="text-subtitle-1">GitOps</span>
      <v-spacer />
      <template v-if="status?.enabled">
        <v-btn
          size="small"
          variant="text"
          prepend-icon="mdi-play"
          :loading="loading"
          @click="handleTrigger"
        >
          Build
        </v-btn>
        <v-btn
          size="small"
          variant="text"
          prepend-icon="mdi-pencil"
          @click="showConfigDialog = true"
        >
          Edit
        </v-btn>
        <v-btn
          size="small"
          variant="text"
          color="error"
          prepend-icon="mdi-close"
          @click="showDisableDialog = true"
        >
          Disable
        </v-btn>
      </template>
      <v-btn
        v-else
        size="small"
        variant="text"
        prepend-icon="mdi-plus"
        @click="showConfigDialog = true"
      >
        Enable
      </v-btn>
    </v-card-title>

    <v-alert v-if="error" type="error" density="compact" class="mx-4 mb-2" closable>
      {{ error }}
    </v-alert>

    <template v-if="status?.enabled && status.config">
      <div class="px-4 pb-2">
        <div class="d-flex align-center ga-3 flex-wrap">
          <v-chip size="small" variant="outlined">
            <v-icon icon="mdi-source-branch" start size="small" />
            {{ status.config.branch }}
          </v-chip>
          <v-chip size="small" variant="outlined" class="text-truncate" style="max-width: 400px">
            <v-icon icon="mdi-link" start size="small" />
            {{ status.config.repo_url }}
          </v-chip>
          <v-chip
            v-if="status.last_build_status"
            size="small"
            :color="statusColor(status.last_build_status)"
            variant="flat"
          >
            <v-icon :icon="statusIcon(status.last_build_status)" start size="small" />
            {{ status.last_build_status }}
          </v-chip>
          <v-chip
            v-else
            size="small"
            color="warning"
            variant="flat"
          >
            <v-icon icon="mdi-timer-sand" start size="small" />
            Awaiting first build
          </v-chip>
          <span v-if="status.last_commit_sha" class="text-caption text-secondary font-weight-medium">
            {{ status.last_commit_sha.slice(0, 7) }}
          </span>
          <span v-if="status.last_build_time" class="text-caption text-secondary">
            {{ formatAge(status.last_build_time, { suffix: " ago" }) }}
          </span>
          <AiFixButton
            v-if="isFailedBuildStatus(status.last_build_status) && status.last_build_job"
            label="Fix build with AI"
            @fix="(agent) => onFixBuild(status!.last_build_job!, status!.last_commit_sha, agent)"
          />
        </div>

        <v-alert
          v-if="status.last_build_error"
          type="error"
          density="compact"
          variant="tonal"
          class="mt-2"
        >
          {{ status.last_build_error }}
        </v-alert>

        <v-card
          v-if="showFixCard"
          variant="outlined"
          class="mt-3 fix-result-card"
        >
          <v-card-title class="d-flex align-center py-2">
            <v-icon icon="mdi-auto-fix" class="mr-2" size="small" />
            <span class="text-body-2">
              AI Build Fix
              <span v-if="fixAgent" class="text-caption text-secondary">
                ({{ agentLabel(fixAgent) }})
              </span>
              <span v-if="fixTargetJob" class="text-caption text-secondary ml-2">
                &middot; job <code>{{ fixTargetJob }}</code>
              </span>
            </span>
            <v-spacer />
            <v-chip
              v-if="fixStreaming"
              size="x-small"
              color="info"
              variant="tonal"
              class="mr-2"
            >
              streaming
            </v-chip>
            <v-chip
              v-if="fixJobStatus"
              :color="fixStatusColor"
              size="x-small"
              variant="flat"
              class="mr-2"
            >
              {{ fixJobStatus }}
            </v-chip>
            <v-btn
              icon="mdi-close"
              size="x-small"
              variant="text"
              @click="dismissFixResult"
            />
          </v-card-title>

          <v-alert
            v-if="fixSubmitError"
            type="error"
            density="compact"
            class="mx-2"
          >
            {{ fixSubmitError }}
          </v-alert>

          <v-progress-linear
            v-if="fixSubmitting || (fixJobStatus && !fixTerminal) || fixStreaming"
            indeterminate
            color="info"
          />

          <div v-if="fixJobDetail" class="px-3 py-1 text-caption text-secondary">
            Job: <code>{{ fixJobDetail.job_name }}</code>
            <span v-if="fixJobDetail.started_at">
              &middot; started {{ fixJobDetail.started_at }}
            </span>
            <span v-if="fixJobDetail.completed_at">
              &middot; completed {{ fixJobDetail.completed_at }}
            </span>
          </div>

          <div v-if="fixOutputDisplay" class="fix-output">
            {{ fixOutputDisplay }}
          </div>
        </v-card>

        <v-btn
          variant="text"
          size="small"
          :prepend-icon="showBuilds ? 'mdi-chevron-up' : 'mdi-chevron-down'"
          class="mt-1"
          @click="toggleBuilds"
        >
          Build History
        </v-btn>

        <v-table v-if="showBuilds" density="compact" class="mt-1">
          <thead>
            <tr>
              <th>Job</th>
              <th>SHA</th>
              <th>Status</th>
              <th>Started</th>
              <th>Completed</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="b in builds" :key="b.job_name">
              <td class="text-caption">{{ b.job_name }}</td>
              <td class="text-caption font-weight-medium">{{ b.commit_sha }}</td>
              <td>
                <v-chip
                  size="x-small"
                  :color="statusColor(b.status === 'succeeded' ? 'success' : b.status)"
                  variant="flat"
                >
                  {{ b.status }}
                </v-chip>
              </td>
              <td class="text-caption text-secondary">{{ formatAge(b.started_at, { suffix: " ago" }) }}</td>
              <td class="text-caption text-secondary">{{ formatAge(b.completed_at, { suffix: " ago" }) }}</td>
              <td>
                <div class="d-flex align-center ga-1">
                  <v-btn
                    size="x-small"
                    variant="text"
                    prepend-icon="mdi-console"
                    @click="viewBuildLogs(b.job_name)"
                  >
                    Logs
                  </v-btn>
                  <AiFixButton
                    v-if="isFailedBuildStatus(b.status)"
                    label="Fix with AI"
                    @fix="(agent) => onFixBuild(b.job_name, b.commit_sha, agent)"
                  />
                </div>
              </td>
            </tr>
            <tr v-if="builds.length === 0">
              <td colspan="6" class="text-center text-secondary">No builds yet</td>
            </tr>
          </tbody>
        </v-table>
      </div>
    </template>

    <div
      v-else-if="status && !status.enabled"
      class="text-center py-4 text-secondary text-body-2"
    >
      GitOps not configured
    </div>

    <GitOpsConfigDialog
      v-model="showConfigDialog"
      :initial-config="status?.config"
        :deployment-name="deploymentName"
      :loading="loading"
      :namespace="namespace"
      @save="handleSaveConfig"
    />

    <ConfirmDialog
      v-model="showDisableDialog"
      title="Disable GitOps"
      message="This will stop watching the repository and remove all GitOps configuration from this deployment."
      confirm-text="Disable"
      :loading="loading"
      @confirm="handleDisable"
    />

    <v-dialog v-model="showLogsDialog" max-width="1200" scrollable @update:model-value="(v) => !v && closeLogsDialog()">
      <v-card>
        <v-card-title class="d-flex align-center">
          <v-icon icon="mdi-console" class="mr-2" size="small" />
          <span class="text-subtitle-1">Build Logs</span>
          <span v-if="logsJobName" class="text-caption text-secondary ml-2">
            {{ logsJobName }}
          </span>
          <v-spacer />
          <v-btn icon="mdi-close" variant="text" size="small" @click="closeLogsDialog" />
        </v-card-title>

        <v-card-text class="pa-2">
          <v-alert v-if="logsError" type="error" density="compact" class="mb-2">
            {{ logsError }}
          </v-alert>
          <div v-if="logsLoading" class="text-center py-4 text-secondary">
            Finding build pod...
          </div>
          <LogViewer
            v-else-if="logsPodName"
            :namespace="namespace"
            :pod-name="logsPodName"
            :pod-phase="logsPodPhase ?? undefined"
          />
        </v-card-text>
      </v-card>
    </v-dialog>
  </v-card>
</template>

<style scoped>
.fix-result-card {
  overflow: hidden;
}

.fix-output {
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
