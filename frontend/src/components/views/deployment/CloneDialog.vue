<script setup lang="ts">
import { ref, watch } from "vue";
import { deploymentsUxApi } from "@/api/deployments_additions";
import { namespacesApi } from "@/api/namespaces";

const props = defineProps<{
  modelValue: boolean;
  namespace: string;
  name: string;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  cloned: [payload: { namespace: string; name: string }];
}>();

const namespaces = ref<string[]>([]);
const targetNamespace = ref<string>("");
const newName = ref<string>("");
const overwrite = ref(false);
const loading = ref(false);
const error = ref<string | null>(null);

// Reset on open so a previous error/success doesn't leak into the next use.
watch(
  () => props.modelValue,
  async (open) => {
    if (!open) return;
    error.value = null;
    targetNamespace.value = props.namespace;
    newName.value = props.name;
    overwrite.value = false;
    try {
      const res = await namespacesApi.list();
      namespaces.value = res.namespaces;
    } catch (e) {
      error.value = e instanceof Error ? e.message : "Failed to load namespaces";
    }
  },
);

const isSameTarget = () =>
  targetNamespace.value === props.namespace && newName.value === props.name;

const handleClone = async () => {
  if (!targetNamespace.value) {
    error.value = "Target namespace is required";
    return;
  }
  if (isSameTarget()) {
    error.value =
      "Choose a different namespace or rename — cannot clone onto the source.";
    return;
  }
  loading.value = true;
  error.value = null;
  try {
    const res = await deploymentsUxApi.clone(props.namespace, props.name, {
      target_namespace: targetNamespace.value,
      new_name: newName.value !== props.name ? newName.value : undefined,
      overwrite: overwrite.value || undefined,
    });
    emit("cloned", {
      namespace: res.target_namespace,
      name: res.target_name,
    });
    emit("update:modelValue", false);
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Clone failed";
  } finally {
    loading.value = false;
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
      <v-card-title>Clone Deployment</v-card-title>
      <v-card-text>
        <div class="text-body-2 mb-3 text-secondary">
          Copy <strong>{{ name }}</strong> from <strong>{{ namespace }}</strong>
          into another namespace. The pod template, resources, probes, and
          labels are preserved; cluster-managed metadata (revision history,
          resourceVersion) is stripped.
        </div>

        <v-alert
          v-if="error"
          type="error"
          density="compact"
          class="mb-3"
          closable
          @click:close="error = null"
        >
          {{ error }}
        </v-alert>

        <v-combobox
          v-model="targetNamespace"
          :items="namespaces"
          label="Target Namespace"
          density="compact"
          class="mb-2"
        />

        <v-text-field
          v-model="newName"
          label="New Name"
          hint="Leave unchanged to keep the source name (only allowed when cloning to a different namespace)"
          persistent-hint
          density="compact"
        />

        <v-switch
          v-model="overwrite"
          label="Overwrite if a deployment with this name already exists"
          color="warning"
          density="compact"
          hide-details
          class="mt-3"
        />
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
          :disabled="!targetNamespace || isSameTarget()"
          @click="handleClone"
        >
          Clone
        </v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>
