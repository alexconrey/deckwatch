<script setup lang="ts">
import { ref, watch, computed } from "vue";
import { deploymentsApi } from "@/api/deployments";
import { cronjobsApi } from "@/api/cronjobs";
import { applicationsApi } from "@/api/applications";
import type {
  ApplicationDetail,
  CronJobSummary,
  DeploymentSummary,
} from "@/types/api";

const props = defineProps<{
  modelValue: boolean;
  namespace: string;
  applicationName: string;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  added: [detail: ApplicationDetail];
}>();

const APP_LABEL = "deckwatch.io/application";

const deployments = ref<DeploymentSummary[]>([]);
const cronjobs = ref<CronJobSummary[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const addingKey = ref<string | null>(null);

const availableDeployments = computed(() =>
  deployments.value.filter((d) => !(d.labels && d.labels[APP_LABEL])),
);

const availableCronjobs = computed(() =>
  cronjobs.value.filter((c) => !(c.labels && c.labels[APP_LABEL])),
);

const fetchCandidates = async () => {
  if (!props.namespace) return;
  loading.value = true;
  error.value = null;
  try {
    const [dRes, cRes] = await Promise.all([
      deploymentsApi.list(props.namespace),
      cronjobsApi.list(props.namespace),
    ]);
    deployments.value = dRes.deployments;
    cronjobs.value = cRes.cronjobs;
  } catch (e) {
    error.value =
      e instanceof Error ? e.message : "Failed to fetch resources";
  } finally {
    loading.value = false;
  }
};

watch(
  () => props.modelValue,
  (open) => {
    if (open) {
      void fetchCandidates();
    }
  },
);

const addMember = async (kind: string, resourceName: string) => {
  addingKey.value = `${kind}/${resourceName}`;
  error.value = null;
  try {
    const detail = await applicationsApi.addMember(
      props.namespace,
      props.applicationName,
      { kind, resource_name: resourceName },
    );
    emit("added", detail);
    await fetchCandidates();
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to add resource";
  } finally {
    addingKey.value = null;
  }
};

const close = () => emit("update:modelValue", false);
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="640"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card>
      <v-card-title class="d-flex align-center">
        <span>Add Resource to Application</span>
        <v-spacer />
        <v-btn icon="mdi-close" variant="text" size="small" @click="close" />
      </v-card-title>

      <v-card-text>
        <v-alert v-if="error" type="error" density="compact" class="mb-3" closable>
          {{ error }}
        </v-alert>

        <div v-if="loading" class="text-center py-6 text-secondary">
          <v-progress-circular indeterminate color="primary" />
          <div class="mt-2 text-body-2">Loading resources...</div>
        </div>

        <template v-else>
          <div class="mb-4">
            <div class="text-subtitle-2 mb-2">Deployments</div>
            <v-table v-if="availableDeployments.length > 0" density="compact">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Image</th>
                  <th style="width: 100px"></th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="d in availableDeployments" :key="d.name">
                  <td class="font-weight-medium">{{ d.name }}</td>
                  <td class="text-caption text-secondary">{{ d.image }}</td>
                  <td>
                    <v-btn
                      size="x-small"
                      color="primary"
                      variant="flat"
                      :loading="addingKey === `Deployment/${d.name}`"
                      :disabled="addingKey !== null"
                      @click="addMember('Deployment', d.name)"
                    >
                      Add
                    </v-btn>
                  </td>
                </tr>
              </tbody>
            </v-table>
            <div
              v-else
              class="text-center py-3 text-secondary text-body-2"
            >
              No unassigned deployments in this namespace
            </div>
          </div>

          <div>
            <div class="text-subtitle-2 mb-2">CronJobs</div>
            <v-table v-if="availableCronjobs.length > 0" density="compact">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Schedule</th>
                  <th style="width: 100px"></th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="c in availableCronjobs" :key="c.name">
                  <td class="font-weight-medium">{{ c.name }}</td>
                  <td><code class="text-caption">{{ c.schedule }}</code></td>
                  <td>
                    <v-btn
                      size="x-small"
                      color="primary"
                      variant="flat"
                      :loading="addingKey === `CronJob/${c.name}`"
                      :disabled="addingKey !== null"
                      @click="addMember('CronJob', c.name)"
                    >
                      Add
                    </v-btn>
                  </td>
                </tr>
              </tbody>
            </v-table>
            <div
              v-else
              class="text-center py-3 text-secondary text-body-2"
            >
              No unassigned cronjobs in this namespace
            </div>
          </div>
        </template>
      </v-card-text>

      <v-card-actions>
        <v-spacer />
        <v-btn variant="text" @click="close">Done</v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>
