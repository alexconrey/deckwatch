<script setup lang="ts">
import { ref } from "vue";
import { useRouter } from "vue-router";
import { podsApi } from "@/api/pods";
import { usePolling } from "@/composables/usePolling";
import type { PodSummary } from "@/types/api";
import PodStatusIcon from "@/components/views/pod/PodStatusIcon.vue";
import { formatAge } from "@/utils/format";

const props = defineProps<{
  namespace: string;
  podName: string;
}>();

const router = useRouter();
const pod = ref<PodSummary | null>(null);
const loading = ref(true);
const error = ref<string | null>(null);

const fetchPod = async () => {
  try {
    pod.value = await podsApi.get(props.namespace, props.podName);
    loading.value = false;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to fetch pod";
    loading.value = false;
  }
};

usePolling(fetchPod, 5000);

const phaseColor = (phase: string): string => {
  if (phase === "Running") return "success";
  if (phase === "Pending") return "info";
  if (phase === "Succeeded") return "success";
  if (phase === "Failed") return "error";
  return "secondary";
};

const containerStateColor = (state: string): string => {
  if (state === "running") return "success";
  if (state === "waiting") return "warning";
  if (state === "terminated") return "error";
  return "secondary";
};
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <v-btn
        icon="mdi-arrow-left"
        variant="text"
        @click="router.back()"
      />
      <div class="ml-2">
        <h2 class="text-h5">{{ podName }}</h2>
        <span class="text-caption text-secondary">{{ namespace }}</span>
      </div>
    </div>

    <v-alert v-if="error" type="error" class="mb-4" closable>
      {{ error }}
    </v-alert>

    <v-progress-linear v-if="loading" indeterminate color="primary" />

    <template v-if="pod">
      <!-- Overview -->
      <v-card class="mb-4 pa-4">
        <div class="d-flex align-center ga-4 flex-wrap">
          <PodStatusIcon :phase="pod.phase" :ready="pod.ready" />
          <v-chip
            size="small"
            variant="flat"
            :color="phaseColor(pod.phase)"
          >
            {{ pod.phase }}
          </v-chip>
          <v-chip
            size="small"
            variant="outlined"
            :color="pod.ready ? 'success' : 'warning'"
          >
            <v-icon
              :icon="pod.ready ? 'mdi-check' : 'mdi-alert'"
              start
              size="small"
            />
            {{ pod.ready ? "Ready" : "Not Ready" }}
          </v-chip>
          <v-chip
            v-if="pod.restart_count > 0"
            size="small"
            variant="flat"
            color="warning"
          >
            <v-icon icon="mdi-restart" start size="small" />
            {{ pod.restart_count }} restart{{ pod.restart_count === 1 ? "" : "s" }}
          </v-chip>
          <v-chip v-else size="small" variant="outlined" color="secondary">
            <v-icon icon="mdi-restart" start size="small" />
            0 restarts
          </v-chip>
          <v-chip size="small" variant="outlined">
            <v-icon icon="mdi-server" start size="small" />
            {{ pod.node ?? "-" }}
          </v-chip>
          <v-chip size="small" variant="outlined" color="secondary">
            <v-icon icon="mdi-clock-outline" start size="small" />
            {{ formatAge(pod.started_at, { suffix: " ago" }) }}
          </v-chip>
        </div>
      </v-card>

      <!-- Containers -->
      <v-card class="mb-4">
        <v-card-title class="text-subtitle-1">
          Containers ({{ pod.container_statuses.length }})
        </v-card-title>
        <v-table
          v-if="pod.container_statuses.length > 0"
          density="compact"
        >
          <thead>
            <tr>
              <th>Name</th>
              <th>Image</th>
              <th>State</th>
              <th>Reason</th>
              <th>Ready</th>
              <th>Restarts</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="cs in pod.container_statuses" :key="cs.name">
              <td class="font-weight-medium">{{ cs.name }}</td>
              <td class="text-caption">
                <v-chip size="x-small" variant="outlined">
                  <v-icon icon="mdi-docker" start size="x-small" />
                  {{ cs.image }}
                </v-chip>
              </td>
              <td>
                <v-chip
                  size="x-small"
                  variant="flat"
                  :color="containerStateColor(cs.state)"
                >
                  {{ cs.state }}
                </v-chip>
              </td>
              <td class="text-caption text-secondary">
                {{ cs.state_reason ?? "-" }}
              </td>
              <td>
                <v-icon
                  :icon="cs.ready ? 'mdi-check' : 'mdi-close'"
                  :color="cs.ready ? 'success' : 'error'"
                  size="small"
                />
              </td>
              <td>
                <v-chip
                  v-if="cs.restart_count > 0"
                  color="warning"
                  size="x-small"
                  variant="flat"
                >
                  {{ cs.restart_count }}
                </v-chip>
                <span v-else class="text-secondary">0</span>
              </td>
            </tr>
          </tbody>
        </v-table>
        <div v-else class="text-center py-4 text-secondary text-body-2">
          No container statuses reported
        </div>
      </v-card>

      <!-- Conditions -->
      <v-card v-if="pod.conditions.length > 0" class="mb-4">
        <v-card-title class="text-subtitle-1">Conditions</v-card-title>
        <v-table density="compact">
          <thead>
            <tr>
              <th>Type</th>
              <th>Status</th>
              <th>Reason</th>
              <th>Message</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="c in pod.conditions" :key="c.condition_type">
              <td>{{ c.condition_type }}</td>
              <td>
                <v-icon
                  :icon="c.status ? 'mdi-check' : 'mdi-close'"
                  :color="c.status ? 'success' : 'error'"
                  size="small"
                />
              </td>
              <td class="text-caption">{{ c.reason ?? "-" }}</td>
              <td class="text-caption text-secondary">
                {{ c.message ?? "-" }}
              </td>
            </tr>
          </tbody>
        </v-table>
      </v-card>
    </template>
  </div>
</template>
