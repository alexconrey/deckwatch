<script setup lang="ts">
import { computed, ref } from "vue";
import { monitoringApi } from "@/api/monitoring";
import { usePolling } from "@/composables/usePolling";
import type {
  MonitorConfigRequest,
  MonitorSettings,
} from "@/types/monitoring";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import MonitorConfigDialog from "@/components/views/deployment/MonitorConfigDialog.vue";

const props = defineProps<{
  namespace: string;
  deploymentName: string;
}>();

const settings = ref<MonitorSettings | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const showConfigDialog = ref(false);
const showDisableDialog = ref(false);

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

const handleSave = async (body: MonitorConfigRequest) => {
  loading.value = true;
  error.value = null;
  try {
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

    <MonitorConfigDialog
      v-model="showConfigDialog"
      :namespace="namespace"
      :enabled="enabled"
      :current="settings"
      :loading="loading"
      @save="handleSave"
    />

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
