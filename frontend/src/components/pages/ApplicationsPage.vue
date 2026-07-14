<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useNamespaceStore } from "@/stores/namespace";
import { useApplicationsStore } from "@/stores/applications";
import { usePolling } from "@/composables/usePolling";
import { applicationsApi } from "@/api/applications";
import { settingsApi } from "@/api/settings";
import ApplicationHealthChip from "@/components/common/ApplicationHealthChip.vue";
import { formatAge } from "@/utils/format";
import { estimateCost, formatCost, type HourlyCost } from "@/utils/cost";
import type { CostSettings } from "@/types/api";

const router = useRouter();
const ns = useNamespaceStore();
const applications = useApplicationsStore();

// --- Cost overlay ---
// Aggregate cost per application is computed by fetching each application's
// detail and summing its deployments' request-based estimates. We only pay
// that N-detail-fetch cost when the operator has actually configured rates —
// otherwise the column is hidden and no requests are issued.
const costSettings = ref<CostSettings | null>(null);
const costByApp = ref<Map<string, HourlyCost | null>>(new Map());
const costLoading = ref(false);

const costEnabled = computed(() => {
  const c = costSettings.value;
  if (!c) return false;
  return c.cost_per_cpu_hour !== null || c.cost_per_gb_hour !== null;
});

onMounted(async () => {
  try {
    const s = await settingsApi.get();
    costSettings.value = s.cost ?? null;
  } catch {
    // Cost overlay is optional; a failure just hides the column.
  }
});

async function loadAppCosts() {
  if (!costEnabled.value || !ns.selected) {
    costByApp.value = new Map();
    return;
  }
  costLoading.value = true;
  const namespace = ns.selected;
  try {
    // Parallel detail fetches. Failed rows resolve to null so a single 404
    // in a fresh app doesn't blank out the whole column.
    const results = await Promise.all(
      applications.applications.map(async (app) => {
        try {
          const detail = await applicationsApi.get(namespace, app.name);
          let hourly = 0;
          let currency = costSettings.value?.currency ?? "USD";
          let contributed = false;
          for (const d of detail.deployments) {
            const est = estimateCost(
              d.resource_requests ?? null,
              d.replicas.desired,
              costSettings.value,
            );
            if (est) {
              hourly += est.hourly;
              currency = est.currency;
              contributed = true;
            }
          }
          const cost: HourlyCost | null = contributed
            ? { hourly, monthly: hourly * 730, currency }
            : null;
          return [app.name, cost] as const;
        } catch {
          return [app.name, null] as const;
        }
      }),
    );
    costByApp.value = new Map(results);
  } finally {
    costLoading.value = false;
  }
}

const refresh = async () => {
  if (ns.selected) {
    await applications.fetchApplications(ns.selected);
    await loadAppCosts();
  }
};

const { refresh: manualRefresh } = usePolling(refresh, 5000);

watch(
  () => ns.selected,
  () => refresh(),
);
// Re-fetch aggregate costs when the overlay is enabled after the app list
// has already loaded (settings-page toggle without a namespace change).
watch(costEnabled, (enabled) => {
  if (enabled) loadAppCosts();
  else costByApp.value = new Map();
});

const headers = computed(() => {
  const base: Array<Record<string, unknown>> = [
    { title: "Name", key: "name" },
    { title: "Description", key: "description" },
    { title: "Health", key: "health", width: "140px" },
    { title: "Deployments", key: "deployment_count", width: "130px", align: "end" as const },
    { title: "CronJobs", key: "cronjob_count", width: "110px", align: "end" as const },
    { title: "GitOps", key: "gitops_enabled", width: "90px", align: "center" as const },
  ];
  if (costEnabled.value) {
    base.push({ title: "Cost", key: "cost", width: "140px", align: "end" as const, sortable: false });
  }
  base.push({ title: "Age", key: "created_at", width: "120px" });
  return base;
});

function costForApp(name: string): string {
  const c = costByApp.value.get(name);
  if (c === undefined) return costLoading.value ? "…" : "—";
  if (c === null) return "—";
  return `${formatCost(c.hourly, c.currency)}/hr`;
}


const goToDetail = (name: string) => {
  router.push({
    name: "ApplicationDetail",
    params: { namespace: ns.selected, name },
  });
};
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <h2 class="text-h5">Applications</h2>
      <v-spacer />
      <v-btn
        variant="text"
        prepend-icon="mdi-refresh"
        :loading="applications.loading || costLoading"
        class="mr-2"
        @click="manualRefresh"
      >
        Refresh
      </v-btn>
      <v-btn
        color="primary"
        prepend-icon="mdi-plus"
        @click="router.push({ name: 'CreateApplication' })"
      >
        Create Application
      </v-btn>
    </div>

    <v-alert v-if="!ns.selected" type="info" class="mb-4">
      Select a namespace to view applications.
    </v-alert>

    <v-alert v-if="applications.error" type="error" class="mb-4" closable>
      {{ applications.error }}
    </v-alert>

    <v-data-table
      v-if="ns.selected"
      :items="applications.applications"
      :headers="headers"
      :loading="applications.loading"
      item-value="name"
      hover
      class="bg-surface rounded"
      @click:row="(_: any, row: any) => goToDetail(row.item.name)"
    >
      <template v-slot:item.name="{ item }">
        <span class="text-body-1 font-weight-medium">{{ item.name }}</span>
      </template>

      <template v-slot:item.description="{ item }">
        <span class="text-body-2 text-secondary">
          {{ item.description || "-" }}
        </span>
      </template>

      <template v-slot:item.health="{ item }">
        <ApplicationHealthChip :health="item.health" />
      </template>

      <template v-slot:item.deployment_count="{ item }">
        <span class="text-body-2">{{ item.deployment_count }}</span>
      </template>

      <template v-slot:item.cronjob_count="{ item }">
        <span class="text-body-2">{{ item.cronjob_count }}</span>
      </template>

      <template v-slot:item.gitops_enabled="{ item }">
        <v-icon
          v-if="item.gitops_enabled"
          icon="mdi-source-branch-check"
          color="success"
          size="small"
        />
        <v-icon
          v-else
          icon="mdi-source-branch-remove"
          color="grey"
          size="small"
        />
      </template>

      <template v-slot:item.cost="{ item }">
        <span class="text-body-2">{{ costForApp(item.name) }}</span>
      </template>

      <template v-slot:item.created_at="{ item }">
        <span class="text-body-2 text-secondary">
          {{ formatAge(item.created_at) }}
        </span>
      </template>

      <template v-slot:no-data>
        <div class="text-center py-8 text-secondary">
          <v-icon icon="mdi-apps" size="48" class="mb-2" />
          <div>No applications in this namespace</div>
          <v-btn
            class="mt-3"
            color="primary"
            variant="tonal"
            size="small"
            prepend-icon="mdi-plus"
            @click="router.push({ name: 'CreateApplication' })"
          >
            Create your first application
          </v-btn>
        </div>
      </template>
    </v-data-table>
  </div>
</template>
