<script setup lang="ts">
import { ref, computed, watch, onUnmounted } from "vue";

const props = defineProps<{
  modelValue: boolean;
  namespace: string;
  podName: string;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: boolean): void;
}>();

const isOpen = computed({
  get: () => props.modelValue,
  set: (v) => emit("update:modelValue", v),
});

type Status = "connecting" | "open" | "closed" | "error";

interface Forward {
  port: number;
  status: Status;
  message: string | null;
  bytesIn: number;
  bytesOut: number;
  socket: WebSocket | null;
  // HTTP proxy is always exposed for any forward -- a plain link that opens
  // the pod's port in a new tab through the deckwatch HTTP proxy endpoint.
  // The WebSocket connection is optional: users only need to open one if they
  // want to see raw bytes / non-HTTP traffic.
  isHttpLike: boolean;
}

// The dialog carries state across open/close inside a single mount so users
// can flip it closed to reference something and reopen without losing their
// active tunnels. All sockets are torn down on component unmount.
const forwards = ref<Forward[]>([]);
const nextPort = ref<number | null>(null);
const errorMessage = ref<string | null>(null);

function proxyUrl(port: number, path = "/"): string {
  const base = `${window.location.origin}/api/namespaces/${encodeURIComponent(
    props.namespace,
  )}/pods/${encodeURIComponent(props.podName)}/proxy/${port}`;
  return path === "/" ? `${base}/` : `${base}${path.startsWith("/") ? path : "/" + path}`;
}

function wsUrl(port: number): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}/api/namespaces/${encodeURIComponent(
    props.namespace,
  )}/pods/${encodeURIComponent(props.podName)}/portforward?port=${port}`;
}

function statusColor(status: Status): string {
  switch (status) {
    case "open":
      return "success";
    case "connecting":
      return "info";
    case "error":
      return "error";
    case "closed":
      return "secondary";
  }
}

function statusLabel(status: Status): string {
  switch (status) {
    case "open":
      return "Connected";
    case "connecting":
      return "Connecting...";
    case "closed":
      return "Disconnected";
    case "error":
      return "Error";
  }
}

function addForward() {
  errorMessage.value = null;
  if (nextPort.value == null || !Number.isFinite(nextPort.value)) {
    errorMessage.value = "Enter a port number";
    return;
  }
  const p = Math.trunc(nextPort.value);
  if (p < 1 || p > 65535) {
    errorMessage.value = "Port must be between 1 and 65535";
    return;
  }
  if (forwards.value.some((f) => f.port === p)) {
    errorMessage.value = `Port ${p} is already listed`;
    return;
  }
  // Heuristic for "should we surface an HTTP link": treat common HTTP ports
  // as HTTP-like by default so the user sees the button, but always allow
  // opening the link regardless -- deckwatch has no way to actually probe
  // the pod's protocol without sending traffic.
  const isHttpLike = isCommonHttpPort(p);
  forwards.value.push({
    port: p,
    status: "closed",
    message: null,
    bytesIn: 0,
    bytesOut: 0,
    socket: null,
    isHttpLike,
  });
  nextPort.value = null;
}

function isCommonHttpPort(p: number): boolean {
  if (p === 80 || p === 443 || p === 8000 || p === 8080 || p === 8443 || p === 3000 || p === 5000 || p === 9000 || p === 9090) return true;
  // Anything in the "web app" ranges people typically use.
  if (p >= 3000 && p <= 3999) return true;
  if (p >= 8000 && p <= 8999) return true;
  return false;
}

function connect(f: Forward) {
  disconnect(f);
  f.status = "connecting";
  f.message = null;
  f.bytesIn = 0;
  f.bytesOut = 0;

  let socket: WebSocket;
  try {
    socket = new WebSocket(wsUrl(f.port));
    socket.binaryType = "arraybuffer";
  } catch (e) {
    f.status = "error";
    f.message = e instanceof Error ? e.message : String(e);
    return;
  }
  f.socket = socket;

  socket.onopen = () => {
    f.status = "open";
  };
  socket.onmessage = (ev) => {
    if (ev.data instanceof ArrayBuffer) {
      f.bytesIn += ev.data.byteLength;
    } else if (typeof ev.data === "string") {
      f.bytesIn += ev.data.length;
    } else if (ev.data instanceof Blob) {
      f.bytesIn += ev.data.size;
    }
  };
  socket.onerror = () => {
    f.status = "error";
    if (!f.message) f.message = "WebSocket error";
  };
  socket.onclose = (ev) => {
    if (f.status !== "error") f.status = "closed";
    if (ev.code !== 1000 && ev.code !== 1005 && !f.message) {
      f.message = `Closed (${ev.code}${ev.reason ? ": " + ev.reason : ""})`;
    }
  };
}

function disconnect(f: Forward) {
  if (f.socket) {
    try {
      f.socket.close();
    } catch {
      // ignore
    }
    f.socket = null;
  }
}

function remove(f: Forward) {
  disconnect(f);
  forwards.value = forwards.value.filter((x) => x !== f);
}

function openInNewTab(port: number) {
  window.open(proxyUrl(port), "_blank", "noopener,noreferrer");
}

async function copyProxyUrl(port: number) {
  try {
    await navigator.clipboard.writeText(proxyUrl(port));
  } catch {
    // Clipboard writes can fail in insecure contexts; the "open in tab"
    // button remains as an escape hatch.
  }
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
}

// When the dialog closes we keep the state around, but we do not need to
// hold sockets open -- users can reconnect on demand when they reopen.
watch(
  () => props.modelValue,
  (open) => {
    if (!open) {
      for (const f of forwards.value) disconnect(f);
    }
  },
);

onUnmounted(() => {
  for (const f of forwards.value) disconnect(f);
});
</script>

<template>
  <v-dialog v-model="isOpen" max-width="820" scrollable>
    <v-card>
      <v-card-title class="d-flex align-center ga-2">
        <v-icon icon="mdi-transit-connection-variant" />
        <span>Port Forward &mdash; {{ podName }}</span>
        <v-spacer />
        <v-btn icon="mdi-close" variant="text" @click="isOpen = false" />
      </v-card-title>

      <v-card-subtitle class="pb-2 text-secondary">
        {{ namespace }} / {{ podName }}
      </v-card-subtitle>

      <v-divider />

      <v-card-text>
        <v-alert v-if="errorMessage" type="error" density="compact" class="mb-3" closable>
          {{ errorMessage }}
        </v-alert>

        <div class="d-flex align-center ga-2 mb-4">
          <v-text-field
            v-model.number="nextPort"
            label="Container port"
            type="number"
            min="1"
            max="65535"
            density="compact"
            hide-details
            style="max-width: 200px"
            @keydown.enter.prevent="addForward"
          />
          <v-btn
            color="primary"
            variant="flat"
            prepend-icon="mdi-plus"
            @click="addForward"
          >
            Add
          </v-btn>
          <span class="text-caption text-secondary ml-2">
            Enter a port exposed by the pod (e.g. 8080, 5432, 6379).
          </span>
        </div>

        <div v-if="forwards.length === 0" class="text-center py-6 text-secondary text-body-2">
          No port-forwards yet. Add one above.
        </div>

        <v-table v-else density="compact">
          <thead>
            <tr>
              <th style="width: 90px">Port</th>
              <th style="width: 140px">Status</th>
              <th>Traffic</th>
              <th>HTTP link</th>
              <th style="width: 220px; text-align: right"></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="f in forwards" :key="f.port">
              <td class="font-weight-medium">{{ f.port }}</td>
              <td>
                <v-chip :color="statusColor(f.status)" size="x-small" variant="flat">
                  {{ statusLabel(f.status) }}
                </v-chip>
                <div v-if="f.message" class="text-caption text-error mt-1">
                  {{ f.message }}
                </div>
              </td>
              <td class="text-caption text-secondary">
                <span title="Bytes received from pod">&darr; {{ fmtBytes(f.bytesIn) }}</span>
                <span class="mx-2">/</span>
                <span title="Bytes sent to pod">&uarr; {{ fmtBytes(f.bytesOut) }}</span>
              </td>
              <td>
                <div class="d-flex align-center ga-1">
                  <v-btn
                    size="x-small"
                    variant="tonal"
                    color="primary"
                    prepend-icon="mdi-open-in-new"
                    @click="openInNewTab(f.port)"
                  >
                    Open
                  </v-btn>
                  <v-btn
                    icon="mdi-content-copy"
                    size="x-small"
                    variant="text"
                    title="Copy proxy URL"
                    @click="copyProxyUrl(f.port)"
                  />
                  <v-chip
                    v-if="!f.isHttpLike"
                    size="x-small"
                    variant="tonal"
                    color="warning"
                    class="ml-1"
                    title="Port is outside the usual HTTP range -- the link will only work if the pod actually speaks HTTP on this port."
                  >
                    non-HTTP?
                  </v-chip>
                </div>
              </td>
              <td style="text-align: right">
                <v-btn
                  v-if="f.status !== 'open' && f.status !== 'connecting'"
                  size="x-small"
                  variant="tonal"
                  color="primary"
                  prepend-icon="mdi-lan-connect"
                  class="mr-1"
                  @click="connect(f)"
                >
                  Connect WS
                </v-btn>
                <v-btn
                  v-else
                  size="x-small"
                  variant="tonal"
                  color="warning"
                  prepend-icon="mdi-lan-disconnect"
                  class="mr-1"
                  @click="disconnect(f)"
                >
                  Disconnect
                </v-btn>
                <v-btn
                  icon="mdi-delete"
                  size="x-small"
                  variant="text"
                  color="error"
                  @click="remove(f)"
                />
              </td>
            </tr>
          </tbody>
        </v-table>

        <v-alert type="info" density="compact" variant="tonal" class="mt-4">
          <div class="text-body-2">
            <strong>HTTP link</strong> proxies your browser through deckwatch to the pod
            port &mdash; use it for admin UIs and REST endpoints.
          </div>
          <div class="text-caption mt-1">
            <strong>Connect WS</strong> opens a raw WebSocket-over-TCP tunnel for the
            port &mdash; useful for non-HTTP protocols (databases, custom TCP services)
            when driven from a client that speaks the WS bridge protocol.
          </div>
        </v-alert>
      </v-card-text>

      <v-divider />

      <v-card-actions>
        <v-spacer />
        <v-btn variant="text" @click="isOpen = false">Close</v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>
