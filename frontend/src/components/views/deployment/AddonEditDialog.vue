<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { AddonDefinition, UpdateAddonRequest } from "@/types/api";

interface EnvRow { name: string; value: string }

const props = defineProps<{
  modelValue: boolean;
  addon: AddonDefinition | null;
  containerName: string;
  // Current effective config as it lives in the pod spec today. Falls back to
  // the addon's defaults when unset so the form is never empty on first open.
  currentPort: number | null;
  currentEnv: EnvRow[];
  currentCpuRequest: string | null;
  currentMemRequest: string | null;
  currentCpuLimit: string | null;
  currentMemLimit: string | null;
  loading?: boolean;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  save: [body: UpdateAddonRequest];
}>();

const port = ref<number | null>(null);
const env = ref<EnvRow[]>([]);
const cpuRequest = ref("");
const memRequest = ref("");
const cpuLimit = ref("");
const memLimit = ref("");

// Re-seed the form each time the dialog opens so a Cancel+reopen shows current
// state, not stale edits from the previous session.
watch(
  () => props.modelValue,
  (open) => {
    if (!open) return;
    port.value = props.currentPort ?? props.addon?.default_port ?? null;
    env.value = props.currentEnv.length > 0
      ? props.currentEnv.map((e) => ({ ...e }))
      : (props.addon?.default_env ?? []).map((e) => ({ name: e.name, value: e.value }));
    const defRes = props.addon?.default_resources ?? null;
    cpuRequest.value = props.currentCpuRequest ?? defRes?.cpu ?? "";
    memRequest.value = props.currentMemRequest ?? defRes?.memory ?? "";
    cpuLimit.value = props.currentCpuLimit ?? defRes?.cpu ?? "";
    memLimit.value = props.currentMemLimit ?? defRes?.memory ?? "";
  },
  { immediate: true },
);

const addEnvRow = () => env.value.push({ name: "", value: "" });
const removeEnvRow = (idx: number) => env.value.splice(idx, 1);

const title = computed(() => {
  const label = props.addon?.name ?? "Addon";
  return `Edit ${label}`;
});

const handleSave = () => {
  const body: UpdateAddonRequest = {};
  if (port.value !== null && port.value !== undefined) body.port = port.value;
  // Always send env when the user opened the dialog, so removing all rows
  // clears the sidecar env list (and un-injects from the primary).
  body.env = env.value
    .filter((e) => e.name.trim().length > 0)
    .map((e) => ({ name: e.name.trim(), value: e.value }));
  const reqCpu = cpuRequest.value.trim();
  const reqMem = memRequest.value.trim();
  if (reqCpu || reqMem) {
    body.resource_requests = { cpu: reqCpu || null, memory: reqMem || null };
  }
  const limCpu = cpuLimit.value.trim();
  const limMem = memLimit.value.trim();
  if (limCpu || limMem) {
    body.resource_limits = { cpu: limCpu || null, memory: limMem || null };
  }
  emit("save", body);
};
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="640"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card>
      <v-card-title class="d-flex align-center">
        <v-icon icon="mdi-puzzle-edit" class="mr-2" />
        {{ title }}
      </v-card-title>
      <v-card-subtitle>container: {{ containerName }}</v-card-subtitle>
      <v-card-text>
        <v-row dense>
          <v-col cols="6">
            <v-text-field
              v-model.number="port"
              label="Port"
              type="number"
              density="compact"
              :placeholder="String(addon?.default_port ?? '')"
            />
          </v-col>
        </v-row>

        <v-divider class="my-3" />
        <div class="d-flex align-center mb-2">
          <div class="text-subtitle-2">Environment Variables</div>
          <v-spacer />
          <v-btn size="small" variant="text" prepend-icon="mdi-plus" @click="addEnvRow">
            Add
          </v-btn>
        </div>
        <div v-if="env.length === 0" class="text-body-2 text-secondary mb-2">
          No environment variables. Removing all will also un-inject them from the primary container.
        </div>
        <v-row v-for="(row, idx) in env" :key="idx" dense align="center">
          <v-col cols="5">
            <v-text-field
              v-model="row.name"
              label="Name"
              density="compact"
              hide-details="auto"
            />
          </v-col>
          <v-col cols="6">
            <v-text-field
              v-model="row.value"
              label="Value"
              density="compact"
              hide-details="auto"
            />
          </v-col>
          <v-col cols="1" class="d-flex justify-end">
            <v-btn
              size="small"
              variant="text"
              color="error"
              icon="mdi-close"
              @click="removeEnvRow(idx)"
            />
          </v-col>
        </v-row>

        <v-divider class="my-3" />
        <div class="text-subtitle-2 mb-2">Resources</div>
        <v-row dense>
          <v-col cols="6">
            <v-text-field
              v-model="cpuRequest"
              label="CPU Request"
              placeholder="100m"
              density="compact"
            />
          </v-col>
          <v-col cols="6">
            <v-text-field
              v-model="memRequest"
              label="Memory Request"
              placeholder="128Mi"
              density="compact"
            />
          </v-col>
          <v-col cols="6">
            <v-text-field
              v-model="cpuLimit"
              label="CPU Limit"
              placeholder="500m"
              density="compact"
            />
          </v-col>
          <v-col cols="6">
            <v-text-field
              v-model="memLimit"
              label="Memory Limit"
              placeholder="256Mi"
              density="compact"
            />
          </v-col>
        </v-row>
      </v-card-text>
      <v-card-actions>
        <v-spacer />
        <v-btn variant="text" @click="emit('update:modelValue', false)">
          Cancel
        </v-btn>
        <v-btn
          color="primary"
          variant="flat"
          :loading="loading"
          @click="handleSave"
        >
          Save
        </v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>
