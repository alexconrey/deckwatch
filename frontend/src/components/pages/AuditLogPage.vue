<script setup lang="ts">
import { computed, ref } from "vue";
import { usePolling } from "@/composables/usePolling";
import { auditApi, type AuditEntry } from "@/api/audit";
import { formatTimestamp } from "@/utils/format";

const entries = ref<AuditEntry[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

const filterResourceType = ref<string | null>(null);
const filterNamespace = ref<string | null>(null);

const resourceTypes = computed(() => {
  const set = new Set(entries.value.map((e) => e.resource_type));
  return Array.from(set).sort();
});

const namespaces = computed(() => {
  const set = new Set(entries.value.map((e) => e.namespace).filter(Boolean));
  return Array.from(set).sort();
});

const refresh = async () => {
  loading.value = true;
  error.value = null;
  try {
    const opts: { resource_type?: string; namespace?: string; limit?: number } = {
      limit: 200,
    };
    if (filterResourceType.value) opts.resource_type = filterResourceType.value;
    if (filterNamespace.value) opts.namespace = filterNamespace.value;
    const response = await auditApi.list(opts);
    entries.value = response.entries;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to fetch audit log";
  } finally {
    loading.value = false;
  }
};

usePolling(refresh, 10000);

const actionColor = (action: string): string => {
  switch (action) {
    case "create":
      return "success";
    case "update":
      return "info";
    case "delete":
      return "error";
    case "scale":
    case "restart":
      return "warning";
    default:
      return "default";
  }
};

const headers = [
  { title: "Timestamp", key: "timestamp", sortable: true },
  { title: "Action", key: "action", sortable: true },
  { title: "Type", key: "resource_type", sortable: true },
  { title: "Name", key: "resource_name", sortable: true },
  { title: "Namespace", key: "namespace", sortable: true },
  { title: "Detail", key: "detail", sortable: false },
];
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <v-icon icon="mdi-clipboard-text-clock" class="mr-2" />
      <span class="text-h5 font-weight-bold">Audit Log</span>
    </div>

    <v-alert v-if="error" type="error" class="mb-4" closable>
      {{ error }}
    </v-alert>

    <v-card flat>
      <v-card-text class="pa-2">
        <div class="d-flex ga-4 mb-4">
          <v-select
            v-model="filterResourceType"
            :items="resourceTypes"
            label="Resource Type"
            density="compact"
            variant="outlined"
            hide-details
            clearable
            style="max-width: 220px"
            @update:model-value="refresh"
          />
          <v-select
            v-model="filterNamespace"
            :items="namespaces"
            label="Namespace"
            density="compact"
            variant="outlined"
            hide-details
            clearable
            style="max-width: 220px"
            @update:model-value="refresh"
          />
        </div>

        <v-data-table
          :headers="headers"
          :items="entries"
          :loading="loading"
          density="compact"
          items-per-page="25"
          :items-per-page-options="[10, 25, 50, 100]"
          hover
        >
          <template #item.timestamp="{ value }">
            {{ formatTimestamp(value) }}
          </template>

          <template #item.action="{ value }">
            <v-chip
              :color="actionColor(value)"
              size="small"
              variant="tonal"
              label
            >
              {{ value }}
            </v-chip>
          </template>

          <template #item.detail="{ value }">
            <span class="text-caption text-medium-emphasis">{{ value }}</span>
          </template>
        </v-data-table>
      </v-card-text>
    </v-card>
  </div>
</template>
