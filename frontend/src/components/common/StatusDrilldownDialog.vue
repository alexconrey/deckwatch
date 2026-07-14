<script setup lang="ts">
import { computed } from "vue";
import type {
  DeploymentCondition,
  DeploymentPhase,
  PodSummary,
} from "@/types/api";

const props = defineProps<{
  modelValue: boolean;
  status: DeploymentPhase;
  conditions: DeploymentCondition[];
  pods: PodSummary[];
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
}>();

interface FailingContainer {
  podName: string;
  containerName: string;
  state: string;
  stateReason: string | null;
  restartCount: number;
}

// Show non-True conditions first so operators see the reason for
// Degraded/Failed at the top; anything that resolved to True is context.
const sortedConditions = computed(() => {
  const rows = [...props.conditions];
  rows.sort((a, b) => {
    const aBad = a.status !== "True" ? 0 : 1;
    const bBad = b.status !== "True" ? 0 : 1;
    return aBad - bBad;
  });
  return rows;
});

const failingContainers = computed<FailingContainer[]>(() => {
  const out: FailingContainer[] = [];
  for (const pod of props.pods) {
    for (const cs of pod.container_statuses) {
      // Ready containers in Running state are healthy — skip them.
      if (cs.ready && cs.state === "running") continue;
      out.push({
        podName: pod.name,
        containerName: cs.name,
        state: cs.state,
        stateReason: cs.state_reason,
        restartCount: cs.restart_count,
      });
    }
  }
  return out;
});

// Distinct reasons across failing containers drive the suggested actions.
const distinctReasons = computed<string[]>(() => {
  const set = new Set<string>();
  for (const c of failingContainers.value) {
    if (c.stateReason) set.add(c.stateReason);
  }
  // Also surface condition reasons that hint at a fix (e.g.
  // FailedCreate, ProgressDeadlineExceeded).
  for (const c of props.conditions) {
    if (c.status !== "True" && c.reason) set.add(c.reason);
  }
  return Array.from(set);
});

interface Suggestion {
  reason: string;
  text: string;
  icon: string;
}

const suggestionFor = (reason: string): Suggestion | null => {
  switch (reason) {
    case "CrashLoopBackOff":
      return {
        reason,
        icon: "mdi-restart-alert",
        text: "Your app is crashing on startup. Check the logs for errors.",
      };
    case "OOMKilled":
      return {
        reason,
        icon: "mdi-memory",
        text: "Your app ran out of memory. Increase the memory limit.",
      };
    case "ImagePullBackOff":
    case "ErrImagePull":
      return {
        reason,
        icon: "mdi-download-off",
        text: "The container image could not be pulled. Verify the image name and registry access.",
      };
    case "CreateContainerConfigError":
      return {
        reason,
        icon: "mdi-cog-off",
        text: "Container configuration error. Check env vars and volume mounts.",
      };
    case "ProgressDeadlineExceeded":
      return {
        reason,
        icon: "mdi-timer-off",
        text: "Rollout exceeded its deadline. Inspect pod events and readiness probes.",
      };
    case "FailedCreate":
      return {
        reason,
        icon: "mdi-alert-octagon",
        text: "Kubernetes could not create pods. Check quotas, PSPs, and the events feed.",
      };
    default:
      return null;
  }
};

const suggestions = computed<Suggestion[]>(() => {
  const out: Suggestion[] = [];
  const seen = new Set<string>();
  for (const reason of distinctReasons.value) {
    const s = suggestionFor(reason);
    if (s && !seen.has(s.reason)) {
      seen.add(s.reason);
      out.push(s);
    }
  }
  return out;
});

const title = computed(() =>
  props.status === "failed"
    ? "Deployment Failed — Diagnostics"
    : "Deployment Degraded — Diagnostics",
);
const headerColor = computed(() =>
  props.status === "failed" ? "error" : "warning",
);
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="720"
    scrollable
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card>
      <v-card-title
        class="d-flex align-center"
        :class="`text-${headerColor}`"
      >
        <v-icon
          :icon="status === 'failed' ? 'mdi-close-circle' : 'mdi-alert'"
          :color="headerColor"
          class="mr-2"
        />
        {{ title }}
        <v-spacer />
        <v-btn
          icon="mdi-close"
          variant="text"
          size="small"
          @click="emit('update:modelValue', false)"
        />
      </v-card-title>

      <v-card-text style="max-height: 70vh">
        <!-- Suggested actions -->
        <div v-if="suggestions.length > 0" class="mb-4">
          <div class="text-subtitle-2 mb-2">Suggested actions</div>
          <v-alert
            v-for="s in suggestions"
            :key="s.reason"
            :icon="s.icon"
            type="info"
            variant="tonal"
            density="compact"
            class="mb-2"
          >
            <div class="font-weight-medium">{{ s.reason }}</div>
            <div class="text-body-2">{{ s.text }}</div>
          </v-alert>
        </div>

        <!-- Conditions -->
        <div v-if="sortedConditions.length > 0" class="mb-4">
          <div class="text-subtitle-2 mb-2">Deployment conditions</div>
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
              <tr v-for="c in sortedConditions" :key="c.condition_type">
                <td>{{ c.condition_type }}</td>
                <td>
                  <v-icon
                    :icon="c.status === 'True' ? 'mdi-check' : 'mdi-close'"
                    :color="c.status === 'True' ? 'success' : 'error'"
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
        </div>

        <!-- Failing containers -->
        <div v-if="failingContainers.length > 0" class="mb-2">
          <div class="text-subtitle-2 mb-2">
            Failing containers ({{ failingContainers.length }})
          </div>
          <v-table density="compact">
            <thead>
              <tr>
                <th>Pod</th>
                <th>Container</th>
                <th>State</th>
                <th>Reason</th>
                <th>Restarts</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="(c, i) in failingContainers"
                :key="`${c.podName}/${c.containerName}/${i}`"
              >
                <td class="text-caption font-weight-medium">
                  {{ c.podName }}
                </td>
                <td class="text-caption">{{ c.containerName }}</td>
                <td>
                  <v-chip size="x-small" variant="flat" color="secondary">
                    {{ c.state }}
                  </v-chip>
                </td>
                <td>
                  <v-chip
                    v-if="c.stateReason"
                    size="x-small"
                    variant="flat"
                    color="error"
                  >
                    {{ c.stateReason }}
                  </v-chip>
                  <span v-else class="text-secondary text-caption">-</span>
                </td>
                <td class="text-caption">{{ c.restartCount }}</td>
              </tr>
            </tbody>
          </v-table>
        </div>

        <v-alert
          v-if="
            sortedConditions.length === 0 &&
            failingContainers.length === 0
          "
          type="info"
          variant="tonal"
          density="compact"
        >
          No condition messages or failing container states were reported.
          Try the Events feed or pod logs for more context.
        </v-alert>
      </v-card-text>

      <v-card-actions>
        <v-spacer />
        <v-btn variant="text" @click="emit('update:modelValue', false)">
          Close
        </v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>
