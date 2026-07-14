<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { usePolling } from "@/composables/usePolling";
import { eventsApi } from "@/api/events";
import type { EventSummary } from "@/types/api";
import { formatAge } from "@/utils/format";

// Reusable K8s events feed. Two modes:
//   * Namespaced: pass `namespace` -- optionally `involvedObject` scopes to a
//     single resource (e.g. a deployment name).
//   * Cluster:    pass `namespace = null` -- lists events across all
//     namespaces the caller is allowed to see (backend filters by allowlist).
//
// The parent controls the source; this component owns fetching, filtering by
// type, rendering, and the 10s poll cadence. Height is capped by
// `maxHeight` so the feed can live inside a page card without pushing other
// content off screen.
const props = withDefaults(
  defineProps<{
    namespace: string | null;
    involvedObject?: string;
    title?: string;
    maxHeight?: number;
    pollMs?: number;
  }>(),
  {
    title: "Events",
    maxHeight: 360,
    pollMs: 10000,
  },
);

type TypeFilter = "all" | "Normal" | "Warning";

const events = ref<EventSummary[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const typeFilter = ref<TypeFilter>("all");
// Default to Warning-only for a cluster-wide feed -- otherwise operators drown
// in "Pulled image" / "Started container" chatter. Detail pages keep "all"
// because on a single deployment the Normal stream is usually short and
// useful.
if (props.namespace === null) {
  typeFilter.value = "Warning";
}

const fetchEvents = async () => {
  loading.value = true;
  error.value = null;
  try {
    const resp = props.namespace === null
      ? await eventsApi.listCluster()
      : await eventsApi.list(props.namespace, {
          involvedObject: props.involvedObject,
        });
    events.value = resp.events;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to fetch events";
  } finally {
    loading.value = false;
  }
};

usePolling(fetchEvents, props.pollMs);

// Re-fetch when the source parameters change (e.g. user switches deployment).
// usePolling only triggers on mount/unmount, so without this the feed would
// stay pinned to the first namespace/object it ever saw.
watch(
  () => [props.namespace, props.involvedObject],
  () => {
    void fetchEvents();
  },
);

const filteredEvents = computed<EventSummary[]>(() => {
  if (typeFilter.value === "all") return events.value;
  return events.value.filter((e) => e.event_type === typeFilter.value);
});

const warningCount = computed(
  () => events.value.filter((e) => e.event_type === "Warning").length,
);
const normalCount = computed(
  () => events.value.filter((e) => e.event_type === "Normal").length,
);

const typeColor = (t: string): string => {
  if (t === "Warning") return "warning";
  if (t === "Normal") return "info";
  return "grey";
};

const typeIcon = (t: string): string => {
  if (t === "Warning") return "mdi-alert";
  if (t === "Normal") return "mdi-information-outline";
  return "mdi-circle-small";
};

const involvedLabel = (e: EventSummary): string => {
  const base = `${e.involved_object_kind}/${e.involved_object_name}`;
  if (props.namespace === null && e.involved_object_namespace) {
    return `${e.involved_object_namespace} · ${base}`;
  }
  return base;
};
</script>

<template>
  <v-card class="mb-4">
    <v-card-title class="d-flex align-center flex-wrap ga-2">
      <v-icon icon="mdi-timeline-text-outline" class="mr-1" />
      <span class="text-subtitle-1">{{ title }}</span>
      <v-chip
        v-if="warningCount > 0"
        size="x-small"
        color="warning"
        variant="tonal"
      >
        {{ warningCount }} warning{{ warningCount === 1 ? "" : "s" }}
      </v-chip>
      <v-chip
        v-if="normalCount > 0"
        size="x-small"
        color="info"
        variant="tonal"
      >
        {{ normalCount }} normal
      </v-chip>
      <v-spacer />
      <v-btn-toggle
        v-model="typeFilter"
        mandatory
        density="compact"
        variant="outlined"
        divided
      >
        <v-btn value="all" size="x-small">All</v-btn>
        <v-btn value="Warning" size="x-small">
          <v-icon icon="mdi-alert" start size="x-small" />
          Warning
        </v-btn>
        <v-btn value="Normal" size="x-small">
          <v-icon icon="mdi-information-outline" start size="x-small" />
          Normal
        </v-btn>
      </v-btn-toggle>
      <v-btn
        icon="mdi-refresh"
        size="small"
        variant="text"
        :loading="loading"
        @click="fetchEvents"
      />
    </v-card-title>

    <v-alert
      v-if="error"
      type="error"
      density="compact"
      variant="tonal"
      class="mx-4 mb-2"
      closable
    >
      {{ error }}
    </v-alert>

    <div
      class="events-feed-body"
      :style="{ maxHeight: `${maxHeight}px` }"
    >
      <template v-if="loading && events.length === 0">
        <div class="text-center py-6 text-secondary">
          <v-progress-circular indeterminate color="primary" size="24" />
          <div class="mt-2 text-body-2">Loading events&hellip;</div>
        </div>
      </template>

      <template v-else-if="filteredEvents.length === 0">
        <div class="text-center py-6 text-secondary">
          <v-icon icon="mdi-check-circle-outline" size="36" class="mb-2" />
          <div v-if="events.length === 0">No events</div>
          <div v-else>No {{ typeFilter }} events</div>
        </div>
      </template>

      <v-list v-else density="compact" class="pa-0">
        <v-list-item
          v-for="ev in filteredEvents"
          :key="`${ev.namespace}/${ev.name}`"
          class="events-feed-row"
          :class="ev.event_type === 'Warning' ? 'event-warning' : 'event-normal'"
        >
          <template v-slot:prepend>
            <v-icon
              :icon="typeIcon(ev.event_type)"
              :color="typeColor(ev.event_type)"
              size="small"
            />
          </template>

          <div class="d-flex align-center flex-wrap ga-2">
            <v-chip
              :color="typeColor(ev.event_type)"
              size="x-small"
              variant="flat"
              label
            >
              {{ ev.reason || ev.event_type }}
            </v-chip>
            <span class="text-caption text-secondary">
              {{ involvedLabel(ev) }}
            </span>
            <v-chip
              v-if="ev.count > 1"
              size="x-small"
              variant="tonal"
              color="secondary"
              label
            >
              &times;{{ ev.count }}
            </v-chip>
            <v-spacer />
            <span
              class="text-caption text-secondary"
              :title="ev.last_timestamp ?? ev.first_timestamp ?? ''"
            >
              {{ formatAge(ev.last_timestamp ?? ev.first_timestamp, { includeSeconds: true, guardInvalid: true }) }}
            </span>
          </div>

          <div class="text-body-2 mt-1 event-message">{{ ev.message }}</div>

          <div
            v-if="ev.source_component"
            class="text-caption text-secondary mt-1"
          >
            source: {{ ev.source_component
            }}<span v-if="ev.source_host"> &middot; {{ ev.source_host }}</span>
          </div>
        </v-list-item>
      </v-list>
    </div>
  </v-card>
</template>

<style scoped>
.events-feed-body {
  overflow-y: auto;
}

.events-feed-row {
  border-left: 3px solid transparent;
  padding-top: 8px;
  padding-bottom: 8px;
}

.events-feed-row.event-warning {
  border-left-color: rgb(var(--v-theme-warning));
}

.events-feed-row.event-normal {
  border-left-color: rgb(var(--v-theme-info));
}

.event-message {
  white-space: pre-wrap;
  word-break: break-word;
}
</style>
