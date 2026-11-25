<script setup lang="ts">
// Enable or edit a Deployment's PodMonitor: the dialog shared by the
// deployment page's Prometheus Monitor card and the application's
// Observability section. The parent saves the request it emits, and closes
// the dialog once that succeeds.
import { computed, ref, watch } from "vue";
import type { MonitorConfigRequest } from "@/types/monitoring";

const props = defineProps<{
  namespace: string;
  /** A PodMonitor exists, so this edits it. */
  enabled: boolean;
  /** Its settings, to start from; null for the defaults. */
  current: { port: string; path: string; interval: string } | null;
  loading?: boolean;
}>();

const open = defineModel<boolean>({ required: true });

const emit = defineEmits<{ save: [body: MonitorConfigRequest] }>();

// Local form state, kept apart from `current` so poll updates don't clobber
// in-progress input while the dialog is open.
const form = ref({
  port: "metrics",
  path: "/metrics",
  interval: "30s",
});

// Prefill from the current settings when opening, so editing an existing
// PodMonitor doesn't silently reset its port/path/interval to the defaults.
watch(open, (isOpen) => {
  if (!isOpen) return;
  form.value = props.current
    ? {
        port: props.current.port || "metrics",
        path: props.current.path || "/metrics",
        interval: props.current.interval || "30s",
      }
    : { port: "metrics", path: "/metrics", interval: "30s" };
});

const canSave = computed(() => {
  if (!form.value.port.trim()) return false;
  if (!form.value.path.trim()) return false;
  if (!/^\d+(ms|s|m|h|d|w|y)$/.test(form.value.interval.trim())) return false;
  return true;
});

function save() {
  emit("save", {
    enabled: true,
    port: form.value.port.trim(),
    path: form.value.path.trim(),
    interval: form.value.interval.trim(),
  });
}
</script>

<template>
  <v-dialog v-model="open" max-width="520">
    <v-card>
      <v-card-title>
        {{ enabled ? "Edit Monitor" : "Enable Monitor" }}
      </v-card-title>
      <v-card-text>
        <div class="text-body-2 mb-3 text-secondary">
          Creates a PodMonitor CRD in namespace
          <code>{{ namespace }}</code> that matches this deployment's pod
          selector. The prometheus-operator will begin scraping the endpoint
          below.
        </div>

        <v-text-field
          v-model="form.port"
          label="Port"
          hint="Named container port (preferred) or numeric string"
          persistent-hint
          density="compact"
          class="mb-2"
        />
        <v-text-field
          v-model="form.path"
          label="Path"
          hint="HTTP path to scrape"
          persistent-hint
          density="compact"
          class="mb-2"
        />
        <v-text-field
          v-model="form.interval"
          label="Scrape interval"
          hint="Prometheus duration, e.g. 30s, 1m, 5m"
          persistent-hint
          density="compact"
        />

        <v-alert
          v-if="!canSave"
          type="warning"
          density="compact"
          variant="tonal"
          class="mt-3"
        >
          Port, path, and a valid Prometheus interval are required.
        </v-alert>
      </v-card-text>
      <v-card-actions>
        <v-spacer />
        <v-btn variant="text" @click="open = false">Cancel</v-btn>
        <v-btn
          color="primary"
          variant="flat"
          :loading="loading"
          :disabled="!canSave"
          @click="save"
        >
          {{ enabled ? "Save" : "Enable" }}
        </v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>
