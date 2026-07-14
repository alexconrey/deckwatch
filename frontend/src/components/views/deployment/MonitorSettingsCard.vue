<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { monitoringApi } from "@/api/monitoring";
import { usePolling } from "@/composables/usePolling";
import type {
  MonitorConfigRequest,
  MonitorSettings,
} from "@/types/monitoring";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";

const props = defineProps<{
  namespace: string;
  deploymentName: string;
}>();

const settings = ref<MonitorSettings | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const showConfigDialog = ref(false);
const showDisableDialog = ref(false);

// Local form state -- kept separate from `settings` so poll updates don't
// clobber in-progress user input while the dialog is open.
const form = ref({
  port: "metrics",
  path: "/metrics",
  interval: "30s",
});

const enabled = computed(() => settings.value?.enabled === true);
const unavailable = computed(() => settings.value?.unavailable_reason ?? null);

const fetchSettings = async () => {
  try {
    settings.value = await monitoringApi.get(
      props.namespace,
      props.deploymentName,
    );
    error.value = null;
  } catch (e) {
    error.value =
      e instanceof Error ? e.message : "Failed to fetch monitor settings";
  }
};

usePolling(fetchSettings, 5000);

// Prefill the dialog from the current settings when opening, so editing an
// existing PodMonitor doesn't silently reset the user's port/path/interval
// back to the defaults.
watch(showConfigDialog, (open) => {
  if (!open) return;
  if (settings.value) {
    form.value = {
      port: settings.value.port || "metrics",
      path: settings.value.path || "/metrics",
      interval: settings.value.interval || "30s",
    };
  } else {
    form.value = { port: "metrics", path: "/metrics", interval: "30s" };
  }
});

const canSave = computed(() => {
  if (!form.value.port.trim()) return false;
  if (!form.value.path.trim()) return false;
  if (!/^\d+(ms|s|m|h|d|w|y)$/.test(form.value.interval.trim())) return false;
  return true;
});

const handleSave = async () => {
  loading.value = true;
  error.value = null;
  try {
    const body: MonitorConfigRequest = {
      enabled: true,
      port: form.value.port.trim(),
      path: form.value.path.trim(),
      interval: form.value.interval.trim(),
    };
    settings.value = await monitoringApi.upsert(
      props.namespace,
      props.deploymentName,
      body,
    );
    showConfigDialog.value = false;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to save monitor";
  } finally {
    loading.value = false;
  }
};

const handleDisable = async () => {
  loading.value = true;
  try {
    await monitoringApi.disable(props.namespace, props.deploymentName);
    await fetchSettings();
    showDisableDialog.value = false;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to disable monitor";
  } finally {
    loading.value = false;
  }
};
</script>

<template>
  <v-card class="mb-4">
    <v-card-title class="d-flex align-center">
      <v-icon icon="mdi-chart-line" class="mr-2" size="small" />
      <span class="text-subtitle-1">Prometheus Monitor</span>
      <v-spacer />
      <template v-if="!unavailable">
        <template v-if="enabled">
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
      </template>
    </v-card-title>

    <v-alert
      v-if="error"
      type="error"
      density="compact"
      class="mx-4 mb-2"
      closable
    >
      {{ error }}
    </v-alert>

    <v-alert
      v-if="unavailable"
      type="warning"
      density="compact"
      variant="tonal"
      class="mx-4 mb-2"
    >
      <div class="text-body-2">{{ unavailable }}</div>
    </v-alert>

    <template v-else-if="enabled && settings">
      <div class="px-4 pb-3">
        <div class="d-flex align-center ga-2 flex-wrap mb-2">
          <v-chip size="small" variant="outlined" color="primary">
            <v-icon icon="mdi-check-circle" start size="small" />
            Scraping enabled
          </v-chip>
          <v-chip size="small" variant="outlined">
            <v-icon icon="mdi-ethernet" start size="small" />
            Port {{ settings.port }}
          </v-chip>
          <v-chip size="small" variant="outlined">
            <v-icon icon="mdi-link-variant" start size="small" />
            {{ settings.path }}
          </v-chip>
          <v-chip size="small" variant="outlined">
            <v-icon icon="mdi-timer-outline" start size="small" />
            Every {{ settings.interval }}
          </v-chip>
          <v-chip
            size="small"
            variant="flat"
            :color="settings.matching_pods > 0 ? 'info' : 'warning'"
          >
            {{ settings.matching_pods }} matching pod{{
              settings.matching_pods === 1 ? "" : "s"
            }}
          </v-chip>
        </div>
        <div class="text-caption text-secondary">
          PodMonitor <code>{{ settings.name }}</code> is present in namespace
          <code>{{ settings.namespace }}</code
          >. Ensure your Prometheus instance's <code>podMonitorSelector</code>
          picks up the label
          <code>app.kubernetes.io/managed-by=deckwatch</code>.
        </div>
      </div>
    </template>

    <div
      v-else-if="!unavailable && settings && !enabled && !error"
      class="text-center py-4 text-secondary text-body-2"
    >
      Prometheus scraping is not configured for this deployment.
    </div>

    <v-dialog v-model="showConfigDialog" max-width="520">
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
          <v-btn variant="text" @click="showConfigDialog = false">Cancel</v-btn>
          <v-btn
            color="primary"
            variant="flat"
            :loading="loading"
            :disabled="!canSave"
            @click="handleSave"
          >
            {{ enabled ? "Save" : "Enable" }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <ConfirmDialog
      v-model="showDisableDialog"
      title="Disable Prometheus Monitor"
      message="This will delete the PodMonitor for this deployment. Prometheus will stop scraping the workload."
      confirm-text="Disable"
      :loading="loading"
      @confirm="handleDisable"
    />
  </v-card>
</template>
