<script setup lang="ts">
import { computed, ref, watch } from "vue";

const props = defineProps<{
  modelValue: boolean;
  title: string;
  message: string;
  confirmText?: string;
  confirmColor?: string;
  loading?: boolean;
  confirmInput?: string;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  confirm: [];
}>();

const typedConfirm = ref("");

// Reset the typed field whenever the dialog opens so a prior aborted
// attempt does not leak in and instantly re-enable the confirm button.
watch(
  () => props.modelValue,
  (open) => {
    if (open) typedConfirm.value = "";
  },
);

const typeMatches = computed(
  () => !props.confirmInput || typedConfirm.value === props.confirmInput,
);
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="420"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card>
      <v-card-title>{{ title }}</v-card-title>
      <v-card-text>
        <div>{{ message }}</div>
        <template v-if="confirmInput">
          <div class="mt-3 text-body-2">
            Type <code>{{ confirmInput }}</code> to confirm:
          </div>
          <v-text-field
            v-model="typedConfirm"
            variant="outlined"
            density="compact"
            hide-details
            autofocus
            class="mt-2"
          />
        </template>
      </v-card-text>
      <v-card-actions>
        <v-spacer />
        <v-btn variant="text" @click="emit('update:modelValue', false)">
          Cancel
        </v-btn>
        <v-btn
          :color="confirmColor ?? 'error'"
          variant="flat"
          :loading="loading"
          :disabled="!typeMatches"
          @click="emit('confirm')"
        >
          {{ confirmText ?? "Confirm" }}
        </v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>
