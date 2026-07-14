<script setup lang="ts">
import { ref, watch, computed } from "vue";
import { promoteApi } from "@/api/promote";
import { namespacesApi } from "@/api/namespaces";
import { ApiError } from "@/api/client";
import type {
  PromoteRequest,
  PromoteResponse,
  PromoteFieldChange,
} from "@/types/api";

const props = defineProps<{
  modelValue: boolean;
  namespace: string;
  name: string;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  promoted: [payload: { namespace: string; name: string }];
}>();

// Two-step flow: user picks target -> "Preview diff" fetches the diff ->
// user confirms -> "Promote" applies. Splitting these keeps the destructive
// action behind a deliberate second click, which is the safety promise of
// the Heroku Pipelines "promote" button we're emulating.
type Step = "pick" | "review";
const step = ref<Step>("pick");

const namespaces = ref<string[]>([]);
const targetNamespace = ref<string>("");
const targetName = ref<string>("");
const changeCause = ref<string>("");

const previewLoading = ref(false);
const applyLoading = ref(false);
const error = ref<string | null>(null);
const preview = ref<PromoteResponse | null>(null);

watch(
  () => props.modelValue,
  async (open) => {
    if (!open) return;
    step.value = "pick";
    error.value = null;
    preview.value = null;
    targetNamespace.value = "";
    targetName.value = props.name;
    changeCause.value = "";
    try {
      const res = await namespacesApi.list();
      // Exclude the source namespace by default — promoting to yourself
      // is a hard error server-side and the dropdown should reflect that.
      namespaces.value = res.namespaces.filter((n) => n !== props.namespace);
    } catch (e) {
      error.value =
        e instanceof Error ? e.message : "Failed to load namespaces";
    }
  },
);

const canFetchPreview = computed(
  () => !!targetNamespace.value && !!targetName.value,
);

async function fetchPreview() {
  if (!canFetchPreview.value) {
    error.value = "Choose a target namespace";
    return;
  }
  previewLoading.value = true;
  error.value = null;
  const body: PromoteRequest = {
    target_namespace: targetNamespace.value,
    target_name: targetName.value !== props.name ? targetName.value : undefined,
    change_cause: changeCause.value.trim() || undefined,
  };
  try {
    preview.value = await promoteApi.preview(props.namespace, props.name, body);
    step.value = "review";
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e.body?.message ?? e.message
        : e instanceof Error
          ? e.message
          : "Failed to preview promotion";
  } finally {
    previewLoading.value = false;
  }
}

async function apply() {
  if (!preview.value) return;
  applyLoading.value = true;
  error.value = null;
  const body: PromoteRequest = {
    target_namespace: preview.value.target_namespace,
    target_name:
      preview.value.target_name !== props.name
        ? preview.value.target_name
        : undefined,
    change_cause: changeCause.value.trim() || undefined,
  };
  try {
    const res = await promoteApi.apply(props.namespace, props.name, body);
    emit("promoted", {
      namespace: res.target_namespace,
      name: res.target_name,
    });
    emit("update:modelValue", false);
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e.body?.message ?? e.message
        : e instanceof Error
          ? e.message
          : "Failed to apply promotion";
  } finally {
    applyLoading.value = false;
  }
}

function backToPick() {
  step.value = "pick";
  preview.value = null;
  error.value = null;
}

// Display helpers. Free-form strings from the backend get shown as-is;
// arrays are formatted as `["a","b"]` to make list-vs-scalar changes
// obvious at a glance.
function displayValue(v: string | null | undefined): string {
  if (v === null || v === undefined || v === "") return "—";
  return v;
}

const changes = computed<PromoteFieldChange[]>(
  () => preview.value?.changes ?? [],
);

function fieldIcon(field: string): string {
  switch (field) {
    case "image":
      return "mdi-docker";
    case "command":
      return "mdi-console";
    case "args":
      return "mdi-code-tags";
    default:
      return "mdi-arrow-right";
  }
}
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="720"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card>
      <v-card-title class="d-flex align-center">
        <v-icon icon="mdi-rocket-launch" class="mr-2" size="small" />
        <span>Promote Deployment</span>
      </v-card-title>

      <v-card-text>
        <div class="text-body-2 mb-4 text-secondary">
          Promote the runnable spec of
          <strong>{{ name }}</strong>
          from
          <strong>{{ namespace }}</strong>
          to another namespace. Only the container
          <em>image</em>, <em>command</em>, and <em>args</em> are copied —
          the target keeps its own replicas, resource limits, env vars, and
          probes so environment-specific tuning is preserved.
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

        <!-- Step 1: pick target -->
        <template v-if="step === 'pick'">
          <v-combobox
            v-model="targetNamespace"
            :items="namespaces"
            label="Target Namespace"
            density="compact"
            class="mb-2"
            hint="A deployment with the target name must already exist in this namespace."
            persistent-hint
          />

          <v-text-field
            v-model="targetName"
            label="Target Deployment Name"
            hint="Defaults to the source name. Change only if the target is renamed across environments."
            persistent-hint
            density="compact"
            class="mt-3 mb-2"
          />

          <v-text-field
            v-model="changeCause"
            label="Change Cause (optional)"
            placeholder="ticket:FSW-7502 or hotfix rationale"
            density="compact"
            hint="Stamped on the target's rollout history for audit."
            persistent-hint
            class="mt-3"
          />
        </template>

        <!-- Step 2: review diff -->
        <template v-else-if="step === 'review' && preview">
          <v-card variant="outlined" class="mb-3">
            <v-card-text class="py-2">
              <div class="d-flex align-center ga-2">
                <v-chip size="small" variant="tonal" color="secondary">
                  {{ preview.source_namespace }} / {{ preview.source_name }}
                </v-chip>
                <v-icon icon="mdi-arrow-right" size="small" />
                <v-chip size="small" variant="tonal" color="primary">
                  {{ preview.target_namespace }} / {{ preview.target_name }}
                </v-chip>
              </div>
            </v-card-text>
          </v-card>

          <div v-if="preview.no_op" class="text-center py-4">
            <v-icon icon="mdi-check-circle" color="success" size="large" />
            <div class="mt-2 text-body-2">
              Source and target are already in sync. Nothing to promote.
            </div>
          </div>

          <div v-else>
            <div class="text-caption text-secondary mb-2">
              {{ changes.length }} field(s) will change:
            </div>
            <v-table density="compact">
              <thead>
                <tr>
                  <th style="width: 90px">Field</th>
                  <th>From (target)</th>
                  <th>To (source)</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="c in changes" :key="c.field">
                  <td>
                    <v-chip size="x-small" variant="outlined">
                      <v-icon :icon="fieldIcon(c.field)" start size="x-small" />
                      {{ c.field }}
                    </v-chip>
                  </td>
                  <td class="text-caption diff-from">
                    {{ displayValue(c.from) }}
                  </td>
                  <td class="text-caption diff-to">
                    {{ displayValue(c.to) }}
                  </td>
                </tr>
              </tbody>
            </v-table>
          </div>
        </template>
      </v-card-text>

      <v-card-actions>
        <v-btn
          v-if="step === 'review'"
          variant="text"
          prepend-icon="mdi-arrow-left"
          @click="backToPick"
        >
          Back
        </v-btn>
        <v-spacer />
        <v-btn variant="text" @click="emit('update:modelValue', false)">
          Cancel
        </v-btn>
        <v-btn
          v-if="step === 'pick'"
          color="primary"
          variant="flat"
          :loading="previewLoading"
          :disabled="!canFetchPreview"
          @click="fetchPreview"
        >
          Preview Diff
        </v-btn>
        <v-btn
          v-else-if="step === 'review' && preview && !preview.no_op"
          color="warning"
          variant="flat"
          :loading="applyLoading"
          @click="apply"
        >
          Promote
        </v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>

<style scoped>
.diff-from {
  background: rgba(244, 67, 54, 0.06);
  font-family: "JetBrains Mono", "Fira Code", monospace;
}
.diff-to {
  background: rgba(76, 175, 80, 0.08);
  font-family: "JetBrains Mono", "Fira Code", monospace;
}
</style>
