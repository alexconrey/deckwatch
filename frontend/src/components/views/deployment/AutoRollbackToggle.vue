<script setup lang="ts">
// Small standalone toggle for the deckwatch.io/auto-rollback annotation.
//
// Kept out of DeploymentForm because that form's payload is
// CreateDeploymentRequest / UpdateDeploymentRequest, neither of which
// carries annotations. Folding it in would either require a new field on
// both shapes or a magic serialization branch. A dedicated toggle keeps the
// form's contract intact and makes the API call surface small and testable.
//
// Reads the current state from the deployment's live annotations (already
// fetched by the parent) and writes via the /auto-rollback endpoint. The
// parent should re-fetch on `update` so the annotation display refreshes.

import { computed, ref, watch } from "vue";
import { apiFetch } from "@/api/client";

const props = defineProps<{
  namespace: string;
  name: string;
  /** The deployment's `metadata.annotations` map, straight from the detail
   *  response. Passing the whole map (not a boolean) means the toggle
   *  reflects any external change (kubectl annotate, YAML editor) without
   *  needing a separate prop. */
  annotations: Record<string, string>;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  update: [enabled: boolean];
}>();

const AUTO_ROLLBACK_KEY = "deckwatch.io/auto-rollback";
const LAST_AUTO_ROLLBACK_KEY = "deckwatch.io/last-auto-rollback";

// Local optimistic state — mirrors the annotation but is updated
// immediately on toggle so the switch doesn't lag the API round-trip.
const local = ref(readInitial(props.annotations));

watch(
  () => props.annotations,
  (v) => {
    // Only overwrite the local value when the annotation actually changes.
    // Prevents a re-render mid-toggle from snapping the switch back to its
    // previous value before the mutation completes.
    const next = readInitial(v);
    if (next !== local.value && !saving.value) {
      local.value = next;
    }
  },
);

const saving = ref(false);
const error = ref<string | null>(null);

const lastRollback = computed(() => props.annotations[LAST_AUTO_ROLLBACK_KEY] || "");

function readInitial(a: Record<string, string>): boolean {
  return a[AUTO_ROLLBACK_KEY] === "true";
}

async function onToggle(next: boolean) {
  local.value = next;
  saving.value = true;
  error.value = null;
  try {
    await apiFetch<{ enabled: boolean }>(
      `/namespaces/${props.namespace}/deployments/${props.name}/auto-rollback`,
      {
        method: "POST",
        body: JSON.stringify({ enabled: next }),
      },
    );
    emit("update", next);
  } catch (e) {
    // Revert the switch so the UI matches the actual cluster state.
    local.value = !next;
    error.value = e instanceof Error ? e.message : "Failed to update auto-rollback";
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <v-card variant="outlined" class="pa-3">
    <div class="d-flex align-center">
      <div class="flex-grow-1">
        <div class="text-subtitle-2">Auto-rollback</div>
        <div class="text-caption text-secondary">
          When enabled, deckwatch rolls this deployment back to its previous
          revision if it stays in a failing or stuck state for more than 5
          minutes.
        </div>
      </div>
      <v-switch
        :model-value="local"
        color="primary"
        hide-details
        density="compact"
        :loading="saving"
        :disabled="disabled || saving"
        @update:model-value="onToggle($event as boolean)"
      />
    </div>
    <v-alert
      v-if="error"
      type="error"
      density="compact"
      class="mt-2"
      closable
      @click:close="error = null"
    >
      {{ error }}
    </v-alert>
    <div v-if="lastRollback" class="text-caption text-secondary mt-2">
      Last auto-rollback: {{ lastRollback }}
    </div>
  </v-card>
</template>
