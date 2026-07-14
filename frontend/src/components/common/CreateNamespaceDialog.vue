<script setup lang="ts">
import { ref, watch } from "vue";
import { namespacesApi } from "@/api/namespaces";
import { ApiError } from "@/api/client";

const props = defineProps<{
  modelValue: boolean;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  created: [name: string];
}>();

const name = ref("");
const labelsText = ref("");
const submitting = ref(false);
const error = ref<string | null>(null);

// RFC 1123 label: lowercase alphanumerics or '-', start/end alphanumeric, max 63.
const namePattern = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;

const nameRules = [
  (v: string) => !!v || "Name is required",
  (v: string) =>
    namePattern.test(v) ||
    "Must be lowercase alphanumeric or '-', start/end with alphanumeric, up to 63 chars",
];

const reset = () => {
  name.value = "";
  labelsText.value = "";
  error.value = null;
  submitting.value = false;
};

watch(
  () => props.modelValue,
  (open) => {
    if (open) reset();
  },
);

const parseLabels = (text: string): Record<string, string> | undefined => {
  const trimmed = text.trim();
  if (!trimmed) return undefined;
  const out: Record<string, string> = {};
  for (const line of trimmed.split(/[\n,]+/)) {
    const eq = line.indexOf("=");
    if (eq === -1) continue;
    const k = line.slice(0, eq).trim();
    const val = line.slice(eq + 1).trim();
    if (k) out[k] = val;
  }
  return Object.keys(out).length ? out : undefined;
};

const close = () => emit("update:modelValue", false);

const submit = async () => {
  if (!namePattern.test(name.value)) {
    error.value = "Invalid namespace name";
    return;
  }
  submitting.value = true;
  error.value = null;
  try {
    const created = await namespacesApi.create({
      name: name.value,
      labels: parseLabels(labelsText.value),
    });
    emit("created", created.name);
    close();
  } catch (e) {
    if (e instanceof ApiError) {
      error.value = e.body.message;
    } else if (e instanceof Error) {
      error.value = e.message;
    } else {
      error.value = "Failed to create namespace";
    }
  } finally {
    submitting.value = false;
  }
};
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="520"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card>
      <v-card-title>Create Namespace</v-card-title>
      <v-card-text>
        <v-alert v-if="error" type="error" class="mb-4" closable>
          {{ error }}
        </v-alert>
        <v-form @submit.prevent="submit">
          <v-text-field
            v-model="name"
            label="Name"
            :rules="nameRules"
            autofocus
            required
          />
          <v-textarea
            v-model="labelsText"
            label="Labels (optional)"
            hint="key=value per line or comma-separated"
            persistent-hint
            rows="3"
          />
        </v-form>
      </v-card-text>
      <v-card-actions>
        <v-spacer />
        <v-btn variant="text" :disabled="submitting" @click="close">
          Cancel
        </v-btn>
        <v-btn
          color="primary"
          variant="flat"
          :loading="submitting"
          @click="submit"
        >
          Create
        </v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>
