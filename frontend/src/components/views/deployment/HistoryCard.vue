<script setup lang="ts">
import { onMounted, ref } from "vue";
import { deploymentsUxApi } from "@/api/deployments_additions";
import type { RevisionSummary } from "@/types/api";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import RevisionDiffDialog from "./RevisionDiffDialog.vue";
import { formatAge } from "@/utils/format";

const props = defineProps<{
  namespace: string;
  name: string;
}>();

const emit = defineEmits<{
  rolledBack: [revision: number];
}>();

const revisions = ref<RevisionSummary[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const rollbackTarget = ref<RevisionSummary | null>(null);
const rollbackLoading = ref(false);
const rollbackError = ref<string | null>(null);
const diffOpen = ref(false);

const fetchHistory = async () => {
  loading.value = true;
  error.value = null;
  try {
    const res = await deploymentsUxApi.history(props.namespace, props.name);
    revisions.value = res.revisions;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to load history";
  } finally {
    loading.value = false;
  }
};

onMounted(fetchHistory);

// Expose a manual refresh for the parent — after edits/scales the caller
// wants to see the new revision appear without a full page reload.
defineExpose({ refresh: fetchHistory });

const confirmRollback = async () => {
  if (!rollbackTarget.value) return;
  rollbackLoading.value = true;
  rollbackError.value = null;
  try {
    await deploymentsUxApi.rollback(props.namespace, props.name, {
      revision: rollbackTarget.value.revision,
    });
    emit("rolledBack", rollbackTarget.value.revision);
    rollbackTarget.value = null;
    await fetchHistory();
  } catch (e) {
    rollbackError.value =
      e instanceof Error ? e.message : "Rollback failed";
  } finally {
    rollbackLoading.value = false;
  }
};

</script>

<template>
  <v-card class="mb-4">
    <v-card-title class="d-flex align-center">
      <span class="text-subtitle-1">Rollout History</span>
      <v-spacer />
      <v-btn
        size="small"
        variant="text"
        prepend-icon="mdi-compare-horizontal"
        :disabled="revisions.length < 2"
        @click="diffOpen = true"
      >
        Compare
      </v-btn>
      <v-btn
        icon="mdi-refresh"
        size="small"
        variant="text"
        :loading="loading"
        @click="fetchHistory"
      />
    </v-card-title>

    <v-alert
      v-if="error"
      type="error"
      density="compact"
      class="mx-4 mb-2"
      closable
      @click:close="error = null"
    >
      {{ error }}
    </v-alert>

    <v-alert
      v-if="rollbackError"
      type="error"
      density="compact"
      class="mx-4 mb-2"
      closable
      @click:close="rollbackError = null"
    >
      {{ rollbackError }}
    </v-alert>

    <v-progress-linear v-if="loading" indeterminate color="primary" />

    <div
      v-else-if="revisions.length === 0"
      class="text-center py-4 text-secondary text-body-2"
    >
      No revisions recorded yet
    </div>

    <v-table v-else density="compact">
      <thead>
        <tr>
          <th style="width: 80px">Rev</th>
          <th>Image</th>
          <th style="width: 90px">Replicas</th>
          <th style="width: 140px">Age</th>
          <th>Change Cause</th>
          <th style="width: 160px"></th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="r in revisions" :key="r.revision">
          <td>
            <v-chip
              size="x-small"
              :color="r.is_current ? 'success' : 'default'"
              variant="flat"
            >
              #{{ r.revision }}
            </v-chip>
          </td>
          <td class="text-caption font-mono">{{ r.image || "-" }}</td>
          <td class="text-caption">
            {{ r.ready_replicas }}/{{ r.replicas }}
          </td>
          <td class="text-caption text-secondary">
            {{ formatAge(r.created_at, { suffix: " ago" }) }}
          </td>
          <td class="text-caption text-secondary">
            {{ r.change_cause ?? "-" }}
          </td>
          <td>
            <v-tooltip
              v-if="r.is_current"
              text="This is the active revision"
              location="start"
            >
              <template #activator="{ props: activatorProps }">
                <span v-bind="activatorProps" class="text-caption text-secondary">
                  active
                </span>
              </template>
            </v-tooltip>
            <v-btn
              v-else
              size="x-small"
              variant="tonal"
              color="warning"
              prepend-icon="mdi-history"
              @click="rollbackTarget = r"
            >
              Roll back
            </v-btn>
          </td>
        </tr>
      </tbody>
    </v-table>

    <ConfirmDialog
      :model-value="!!rollbackTarget"
      title="Roll back deployment"
      :message="
        rollbackTarget
          ? `Roll ${name} back to revision #${rollbackTarget.revision} (${rollbackTarget.image})? This will trigger a new rollout using the container spec from that revision.`
          : ''
      "
      confirm-text="Roll back"
      confirm-color="warning"
      :loading="rollbackLoading"
      @update:model-value="rollbackTarget = null"
      @confirm="confirmRollback"
    />

    <RevisionDiffDialog
      v-model="diffOpen"
      :namespace="namespace"
      :name="name"
      :revisions="revisions"
    />
  </v-card>
</template>

<style scoped>
.font-mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}
</style>
