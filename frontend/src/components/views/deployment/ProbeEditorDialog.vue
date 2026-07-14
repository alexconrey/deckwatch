<script setup lang="ts">
import type { ProbeConfig, UpdateProbesRequest } from "@/types/api";
import ProbeEditor from "./ProbeEditor.vue";

defineProps<{
  modelValue: boolean;
  liveness: ProbeConfig | null;
  readiness: ProbeConfig | null;
  startup: ProbeConfig | null;
  loading?: boolean;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  save: [body: UpdateProbesRequest];
}>();
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="720"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card>
      <v-card-title class="d-flex align-center">
        <v-icon icon="mdi-heart-pulse" class="mr-2" />
        Health Checks
      </v-card-title>
      <v-card-text>
        <ProbeEditor
          :liveness="liveness"
          :readiness="readiness"
          :startup="startup"
          :loading="loading"
          @save="emit('save', $event)"
          @cancel="emit('update:modelValue', false)"
        />
      </v-card-text>
    </v-card>
  </v-dialog>
</template>
