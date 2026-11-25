<script setup lang="ts">
import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import { pluginStatusApi } from "@/api/pluginStatus";
import { usePolling } from "@/composables/usePolling";
import type { PluginStatus } from "@/types/api";
import PluginHealthChip from "@/components/common/PluginHealthChip.vue";
import PluginEventList from "@/components/common/PluginEventList.vue";
import { formatAge } from "@/utils/format";
import { statusTarget } from "@/utils/pluginEvents";

// Every plugin call target that is failing or degraded right now, with the
// latest events of each. The backend keeps current state only, so a target
// disappears from here once its plugin recovers.
const router = useRouter();
const problems = ref<PluginStatus[]>([]);
const loaded = ref(false);
const error = ref<string | null>(null);

const fetchProblems = async () => {
  try {
    problems.value = await pluginStatusApi.problems();
    error.value = null;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to load plugin problems";
  } finally {
    loaded.value = true;
  }
};

usePolling(fetchProblems, 10000);

const failingCount = computed(() => problems.value.filter((p) => p.health === "failing").length);
const degradedCount = computed(() => problems.value.filter((p) => p.health === "degraded").length);

const ago = (iso: string | null) => formatAge(iso, { guardInvalid: true, includeSeconds: true });

function open(s: PluginStatus) {
  if (s.application) {
    router.push({ name: "ApplicationDetail", params: { namespace: s.namespace, name: s.application } });
  } else {
    router.push({ name: "plugin-settings", params: { name: s.plugin } });
  }
}
</script>

<template>
  <div>
    <div class="d-flex align-center flex-wrap ga-2 mb-4">
      <h2 class="text-h5">Plugin problems</h2>
      <v-chip v-if="failingCount" color="error" size="small" variant="tonal">
        {{ failingCount }} failing
      </v-chip>
      <v-chip v-if="degradedCount" color="warning" size="small" variant="tonal">
        {{ degradedCount }} degraded
      </v-chip>
    </div>

    <v-alert v-if="error" type="error" variant="tonal" class="mb-4">{{ error }}</v-alert>
    <v-progress-linear v-if="!loaded" indeterminate color="primary" class="mb-4" />

    <div
      v-else-if="problems.length === 0 && !error"
      class="text-center py-8 text-secondary text-body-2"
      data-testid="no-plugin-problems"
    >
      No plugin is failing or degraded.
    </div>

    <v-card
      v-for="p in problems"
      :key="`${p.namespace}/${p.application}/${p.plugin}/${p.resource_id}`"
      class="mb-3"
      variant="outlined"
      data-testid="plugin-problem"
    >
      <v-card-title class="d-flex align-center flex-wrap ga-2">
        <PluginHealthChip :health="p.health" />
        <span class="text-subtitle-1 font-weight-medium">{{ p.plugin }}</span>
        <span class="text-body-2 text-secondary">{{ statusTarget(p) }}</span>
        <v-spacer />
        <v-btn size="small" variant="text" @click="open(p)">
          {{ p.application ? "Open application" : "Open plugin" }}
        </v-btn>
      </v-card-title>
      <v-card-text>
        <div class="text-caption text-secondary mb-2">
          <template v-if="p.health === 'failing'">
            Failing for {{ ago(p.first_failure_at) }} ·
            {{ p.consecutive_failures }} failed {{ p.last_export }}() call{{ p.consecutive_failures === 1 ? "" : "s" }} in a row
          </template>
          <template v-else>Last {{ p.last_export }}() call {{ ago(p.updated_at) }} ago</template>
          <template v-if="p.last_success_at"> · last success {{ ago(p.last_success_at) }} ago</template>
          <template v-else> · no success recorded</template>
        </div>
        <PluginEventList :events="p.last_events" hide-plugin />
      </v-card-text>
    </v-card>
  </div>
</template>
