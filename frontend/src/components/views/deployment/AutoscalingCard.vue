<script setup lang="ts">
import { ref, computed, watch } from "vue";
import { autoscalingApi } from "@/api/autoscaling";
import { usePolling } from "@/composables/usePolling";
import type { HpaConfigRequest, HpaResponse } from "@/types/api";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";

const props = defineProps<{
  namespace: string;
  deploymentName: string;
}>();

const hpa = ref<HpaResponse | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const showConfigDialog = ref(false);
const showDisableDialog = ref(false);

// Local form state. Kept separate from `hpa` so the user can edit without
// the poll clobbering their in-progress inputs.
const form = ref({
  minReplicas: 1,
  maxReplicas: 3,
  cpuEnabled: true,
  cpuTarget: 80,
  memoryEnabled: false,
  memoryTarget: 80,
});

const enabled = computed(() => hpa.value !== null);

const fetchHpa = async () => {
  try {
    hpa.value = await autoscalingApi.get(
      props.namespace,
      props.deploymentName,
    );
    error.value = null;
  } catch (e) {
    error.value =
      e instanceof Error ? e.message : "Failed to fetch autoscaling config";
  }
};

usePolling(fetchHpa, 5000);

// Prefill the dialog from the current HPA when opening it -- otherwise the
// form always shows the defaults, which would silently overwrite an existing
// config the user might have opened just to tweak.
watch(showConfigDialog, (open) => {
  if (!open) return;
  if (hpa.value) {
    form.value = {
      minReplicas: hpa.value.min_replicas ?? 1,
      maxReplicas: hpa.value.max_replicas,
      cpuEnabled: hpa.value.target_cpu_utilization !== null,
      cpuTarget: hpa.value.target_cpu_utilization ?? 80,
      memoryEnabled: hpa.value.target_memory_utilization !== null,
      memoryTarget: hpa.value.target_memory_utilization ?? 80,
    };
  } else {
    form.value = {
      minReplicas: 1,
      maxReplicas: 3,
      cpuEnabled: true,
      cpuTarget: 80,
      memoryEnabled: false,
      memoryTarget: 80,
    };
  }
});

const canSave = computed(() => {
  if (form.value.minReplicas < 1) return false;
  if (form.value.maxReplicas < form.value.minReplicas) return false;
  if (!form.value.cpuEnabled && !form.value.memoryEnabled) return false;
  return true;
});

const handleSave = async () => {
  loading.value = true;
  error.value = null;
  try {
    const body: HpaConfigRequest = {
      min_replicas: form.value.minReplicas,
      max_replicas: form.value.maxReplicas,
      target_cpu_utilization: form.value.cpuEnabled
        ? form.value.cpuTarget
        : null,
      target_memory_utilization: form.value.memoryEnabled
        ? form.value.memoryTarget
        : null,
    };
    hpa.value = await autoscalingApi.upsert(
      props.namespace,
      props.deploymentName,
      body,
    );
    showConfigDialog.value = false;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to save HPA";
  } finally {
    loading.value = false;
  }
};

const handleDisable = async () => {
  loading.value = true;
  try {
    await autoscalingApi.delete(props.namespace, props.deploymentName);
    hpa.value = null;
    showDisableDialog.value = false;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to disable HPA";
  } finally {
    loading.value = false;
  }
};

const conditionColor = (status: string) => {
  switch (status) {
    case "True":
      return "success";
    case "False":
      return "error";
    default:
      return "secondary";
  }
};
</script>

<template>
  <v-card class="mb-4">
    <v-card-title class="d-flex align-center">
      <v-icon icon="mdi-arrow-expand-vertical" class="mr-2" size="small" />
      <span class="text-subtitle-1">Autoscaling</span>
      <v-spacer />
      <template v-if="enabled">
        <v-btn size="small" variant="text" prepend-icon="mdi-pencil" @click="showConfigDialog = true">Edit</v-btn>
        <v-btn size="small" variant="text" color="error" prepend-icon="mdi-close" @click="showDisableDialog = true">Disable</v-btn>
      </template>
      <v-btn v-else size="small" variant="text" prepend-icon="mdi-plus" @click="showConfigDialog = true">Enable</v-btn>
    </v-card-title>

    <v-alert v-if="error" type="error" density="compact" class="mx-4 mb-2" closable>{{ error }}</v-alert>

    <template v-if="enabled && hpa">
      <div class="px-4 pb-3">
        <div class="d-flex align-center ga-2 flex-wrap mb-2">
          <v-chip size="small" variant="outlined"><v-icon icon="mdi-arrow-collapse" start size="small" />Min {{ hpa.min_replicas ?? "-" }}</v-chip>
          <v-chip size="small" variant="outlined"><v-icon icon="mdi-arrow-expand" start size="small" />Max {{ hpa.max_replicas }}</v-chip>
          <v-chip v-if="hpa.target_cpu_utilization !== null" size="small" variant="outlined" color="primary">
            <v-icon icon="mdi-chip" start size="small" />CPU {{ hpa.current_cpu_utilization ?? "-" }}% / {{ hpa.target_cpu_utilization }}%
          </v-chip>
          <v-chip v-if="hpa.target_memory_utilization !== null" size="small" variant="outlined" color="secondary">
            <v-icon icon="mdi-memory" start size="small" />Mem {{ hpa.current_memory_utilization ?? "-" }}% / {{ hpa.target_memory_utilization }}%
          </v-chip>
          <v-chip size="small" variant="flat" color="info">{{ hpa.current_replicas ?? "?" }} / {{ hpa.desired_replicas ?? "?" }} replicas</v-chip>
        </div>

        <v-table v-if="hpa.conditions.length > 0" density="compact" class="text-caption">
          <thead>
            <tr>
              <th style="width: 160px">Condition</th>
              <th style="width: 80px">Status</th>
              <th style="width: 140px">Reason</th>
              <th>Message</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="c in hpa.conditions" :key="c.condition_type">
              <td>{{ c.condition_type }}</td>
              <td><v-chip :color="conditionColor(c.status)" size="x-small" variant="flat">{{ c.status }}</v-chip></td>
              <td class="text-secondary">{{ c.reason ?? "-" }}</td>
              <td class="text-secondary">{{ c.message ?? "-" }}</td>
            </tr>
          </tbody>
        </v-table>
      </div>
    </template>

    <div v-else-if="hpa === null && !error" class="text-center py-4 text-secondary text-body-2">
      Manual scaling -- autoscaling not configured
    </div>

    <v-dialog v-model="showConfigDialog" max-width="520">
      <v-card>
        <v-card-title>{{ enabled ? "Edit Autoscaling" : "Enable Autoscaling" }}</v-card-title>
        <v-card-text>
          <v-row dense>
            <v-col cols="6">
              <v-text-field v-model.number="form.minReplicas" label="Min Replicas" type="number" min="1" />
            </v-col>
            <v-col cols="6">
              <v-text-field v-model.number="form.maxReplicas" label="Max Replicas" type="number" :min="form.minReplicas" />
            </v-col>
          </v-row>

          <v-divider class="my-3" />
          <div class="text-body-2 mb-2">Metric Targets</div>

          <div class="d-flex align-center ga-3 mb-2">
            <v-switch v-model="form.cpuEnabled" label="CPU" density="compact" hide-details color="primary" style="max-width: 120px" />
            <v-text-field v-model.number="form.cpuTarget" label="Target CPU %" type="number" min="1" max="100" density="compact" hide-details :disabled="!form.cpuEnabled" suffix="%" style="max-width: 180px" />
          </div>

          <div class="d-flex align-center ga-3">
            <v-switch v-model="form.memoryEnabled" label="Memory" density="compact" hide-details color="secondary" style="max-width: 120px" />
            <v-text-field v-model.number="form.memoryTarget" label="Target Memory %" type="number" min="1" max="100" density="compact" hide-details :disabled="!form.memoryEnabled" suffix="%" style="max-width: 180px" />
          </div>

          <v-alert v-if="!form.cpuEnabled && !form.memoryEnabled" type="warning" density="compact" variant="tonal" class="mt-3">
            At least one metric target must be enabled.
          </v-alert>
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="showConfigDialog = false">Cancel</v-btn>
          <v-btn color="primary" variant="flat" :loading="loading" :disabled="!canSave" @click="handleSave">
            {{ enabled ? "Save" : "Enable" }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <ConfirmDialog
      v-model="showDisableDialog"
      title="Disable Autoscaling"
      message="This will delete the HorizontalPodAutoscaler for this deployment. The current replica count will be preserved, but Kubernetes will no longer adjust it automatically."
      confirm-text="Disable"
      :loading="loading"
      @confirm="handleDisable"
    />
  </v-card>
</template>
