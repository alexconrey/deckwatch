<script setup lang="ts">
import { ref, computed, watch, nextTick, onUnmounted } from "vue";
import type { ContainerStatusSummary } from "@/types/api";

const props = defineProps<{
  modelValue: boolean;
  namespace: string;
  podName: string;
  containers: ContainerStatusSummary[];
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: boolean): void;
}>();

const isOpen = computed({
  get: () => props.modelValue,
  set: (v) => emit("update:modelValue", v),
});

const MAX_BUFFER_CHARS = 200_000;
const TRIM_TO_CHARS = 150_000;

type Status = "idle" | "connecting" | "open" | "closed" | "error";

const status = ref<Status>("idle");
const statusMessage = ref<string | null>(null);
const output = ref("");
const currentInput = ref("");
const outputEl = ref<HTMLElement | null>(null);
const autoScroll = ref(true);

const selectedContainer = ref<string | null>(null);
const commandChoice = ref<"/bin/sh" | "/bin/bash">("/bin/sh");

let socket: WebSocket | null = null;

const statusColor = computed(() => {
  switch (status.value) {
    case "open":
      return "success";
    case "connecting":
      return "info";
    case "error":
      return "error";
    case "closed":
      return "secondary";
    default:
      return "secondary";
  }
});

const statusLabel = computed(() => {
  switch (status.value) {
    case "connecting":
      return "Connecting...";
    case "open":
      return "Connected";
    case "closed":
      return "Disconnected";
    case "error":
      return statusMessage.value ?? "Error";
    default:
      return "Idle";
  }
});

const wsUrl = computed(() => {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  const params = new URLSearchParams();
  if (selectedContainer.value) params.set("container", selectedContainer.value);
  params.set("command", commandChoice.value);
  return `${proto}//${window.location.host}/api/namespaces/${props.namespace}/pods/${props.podName}/exec?${params}`;
});

function appendOutput(chunk: string) {
  output.value += chunk;
  if (output.value.length > MAX_BUFFER_CHARS) {
    output.value = output.value.slice(-TRIM_TO_CHARS);
  }
  if (autoScroll.value) {
    nextTick(() => {
      if (outputEl.value) {
        outputEl.value.scrollTop = outputEl.value.scrollHeight;
      }
    });
  }
}

function connect() {
  disconnect();
  status.value = "connecting";
  statusMessage.value = null;
  output.value = "";

  const s = new WebSocket(wsUrl.value);
  socket = s;

  s.onopen = () => {
    status.value = "open";
  };

  s.onmessage = (ev) => {
    if (typeof ev.data === "string") {
      appendOutput(ev.data);
    } else if (ev.data instanceof Blob) {
      ev.data.text().then(appendOutput);
    }
  };

  s.onerror = () => {
    status.value = "error";
    statusMessage.value = "WebSocket error";
  };

  s.onclose = (ev) => {
    // If we already flagged an error, keep that state.
    if (status.value !== "error") {
      status.value = "closed";
    }
    if (ev.code !== 1000 && ev.code !== 1005 && !statusMessage.value) {
      statusMessage.value = `Closed (${ev.code}${ev.reason ? ": " + ev.reason : ""})`;
    }
  };
}

function disconnect() {
  if (socket) {
    try {
      socket.close();
    } catch {
      // ignore
    }
    socket = null;
  }
}

function sendInput() {
  if (!socket || socket.readyState !== WebSocket.OPEN) return;
  // Servers running `sh` expect a trailing newline to execute the line.
  socket.send(currentInput.value + "\n");
  currentInput.value = "";
}

function sendSignal(byte: string) {
  if (!socket || socket.readyState !== WebSocket.OPEN) return;
  socket.send(byte);
}

function retryWithBash() {
  commandChoice.value = "/bin/bash";
  connect();
}

// Initialize container selection whenever the dialog opens. Also reset when the
// pod itself changes so the previous pod's container name doesn't leak in.
watch(
  () => [props.modelValue, props.podName] as const,
  ([open, _pod], prev) => {
    const prevOpen = prev?.[0] ?? false;
    const prevPod = prev?.[1];
    if (open) {
      // Reset container selection when the dialog opens or the pod changes.
      if (!prevOpen || prevPod !== props.podName) {
        selectedContainer.value =
          props.containers.length > 0 ? props.containers[0].name : null;
        commandChoice.value = "/bin/sh";
        connect();
      }
    } else {
      disconnect();
    }
  },
);

// Reconnect when the user picks a different container while the dialog is open.
watch(selectedContainer, (name, prev) => {
  if (props.modelValue && name && name !== prev) connect();
});

onUnmounted(disconnect);
</script>

<template>
  <v-dialog v-model="isOpen" max-width="1000" scrollable persistent>
    <v-card>
      <v-card-title class="d-flex align-center ga-2">
        <v-icon icon="mdi-console" />
        <span>Terminal — {{ podName }}</span>
        <v-chip :color="statusColor" size="small" variant="flat" class="ml-2">
          {{ statusLabel }}
        </v-chip>
        <v-spacer />
        <v-btn icon="mdi-close" variant="text" @click="isOpen = false" />
      </v-card-title>

      <v-card-subtitle class="d-flex align-center ga-3 pb-2">
        <v-select
          v-if="containers.length > 1"
          v-model="selectedContainer"
          :items="containers.map(c => c.name)"
          label="Container"
          density="compact"
          hide-details
          style="max-width: 260px"
        />
        <span v-else-if="selectedContainer" class="text-caption text-secondary">
          Container: {{ selectedContainer }}
        </span>
        <span class="text-caption text-secondary">Command: {{ commandChoice }}</span>
        <v-btn
          v-if="status === 'error' || (status === 'closed' && commandChoice === '/bin/sh')"
          size="small"
          variant="tonal"
          @click="retryWithBash"
        >
          Try /bin/bash
        </v-btn>
        <v-btn
          v-if="status === 'closed' || status === 'error'"
          size="small"
          variant="tonal"
          color="primary"
          @click="connect"
        >
          Reconnect
        </v-btn>
      </v-card-subtitle>

      <v-divider />

      <v-card-text class="pa-0" style="height: 520px; display: flex; flex-direction: column;">
        <pre
          ref="outputEl"
          class="terminal-output"
        >{{ output }}</pre>

        <div class="terminal-input-row">
          <span class="terminal-prompt">$</span>
          <input
            v-model="currentInput"
            class="terminal-input"
            type="text"
            :disabled="status !== 'open'"
            placeholder="Type a command and press Enter..."
            autocomplete="off"
            spellcheck="false"
            @keydown.enter.prevent="sendInput"
          />
          <v-btn
            size="x-small"
            variant="tonal"
            :disabled="status !== 'open'"
            title="Send Ctrl-C"
            @click="sendSignal(String.fromCharCode(0x03))"
          >
            Ctrl-C
          </v-btn>
          <v-btn
            size="x-small"
            variant="tonal"
            :disabled="status !== 'open'"
            title="Send Ctrl-D (EOF)"
            @click="sendSignal(String.fromCharCode(0x04))"
          >
            Ctrl-D
          </v-btn>
        </div>
      </v-card-text>

      <v-divider />

      <v-card-actions>
        <v-checkbox
          v-model="autoScroll"
          label="Auto-scroll"
          density="compact"
          hide-details
        />
        <v-spacer />
        <v-btn variant="text" @click="output = ''">Clear</v-btn>
        <v-btn color="primary" variant="tonal" @click="isOpen = false">Close</v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>

<style scoped>
.terminal-output {
  flex: 1 1 auto;
  margin: 0;
  padding: 12px 16px;
  background: #0d1117;
  color: #c9d1d9;
  font-family: "Menlo", "Consolas", "Liberation Mono", monospace;
  font-size: 13px;
  line-height: 1.4;
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-all;
}

.terminal-input-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: #161b22;
  border-top: 1px solid #30363d;
}

.terminal-prompt {
  color: #58a6ff;
  font-family: "Menlo", "Consolas", "Liberation Mono", monospace;
  font-weight: bold;
}

.terminal-input {
  flex: 1 1 auto;
  background: transparent;
  border: none;
  outline: none;
  color: #c9d1d9;
  font-family: "Menlo", "Consolas", "Liberation Mono", monospace;
  font-size: 13px;
}

.terminal-input:disabled {
  opacity: 0.5;
}
</style>
