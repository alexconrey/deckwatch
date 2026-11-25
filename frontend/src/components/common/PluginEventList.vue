<script setup lang="ts">
import { computed } from "vue";
import type { PluginEventReport } from "@/types/api";
import { severityColor, severityIcon, sortEvents } from "@/utils/pluginEvents";

// Plugin events are plugin-supplied text. Everything here is rendered with
// text interpolation — never v-html — so markup in a message stays inert.
const props = withDefaults(
  defineProps<{
    events: PluginEventReport[];
    /** Drop info events (e.g. when the caller shows the message itself). */
    hideInfo?: boolean;
    /** Omit the plugin name (when the context already names the plugin). */
    hidePlugin?: boolean;
  }>(),
  { hideInfo: false, hidePlugin: false },
);

const shown = computed(() =>
  sortEvents(props.events).filter((e) => !props.hideInfo || e.severity !== "info"),
);
</script>

<template>
  <div v-if="shown.length" class="plugin-event-list" data-testid="plugin-event-list">
    <div
      v-for="(e, i) in shown"
      :key="`${e.plugin}-${e.code}-${i}`"
      class="d-flex ga-2 py-1 plugin-event"
      :data-severity="e.severity"
    >
      <v-icon
        :icon="severityIcon(e.severity)"
        :color="severityColor(e.severity)"
        size="small"
        class="mt-1"
      />
      <div class="flex-grow-1" style="min-width: 0">
        <div class="text-caption text-medium-emphasis d-flex flex-wrap align-center ga-1">
          <span v-if="!hidePlugin" class="font-weight-medium">{{ e.plugin }}</span>
          <span v-if="!hidePlugin">·</span>
          <span>{{ e.export }}</span>
          <span>·</span>
          <code class="plugin-event-code">{{ e.code }}</code>
          <v-chip v-if="e.retryable" size="x-small" variant="tonal" color="info">
            retryable
          </v-chip>
        </div>
        <div class="text-body-2 plugin-event-message">{{ e.message }}</div>
        <div v-if="e.hint" class="text-caption">
          <span class="font-weight-medium">Hint:</span> {{ e.hint }}
        </div>
        <div v-if="e.resource_id || e.object" class="text-caption text-medium-emphasis">
          <span v-if="e.resource_id">Resource: {{ e.resource_id }}</span>
          <span v-if="e.resource_id && e.object"> · </span>
          <span v-if="e.object">{{ e.object.kind }} {{ e.object.name }}</span>
        </div>
        <div v-if="e.trace_id" class="text-caption text-medium-emphasis">
          Trace: <code>{{ e.trace_id }}</code>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.plugin-event-message {
  overflow-wrap: anywhere;
}
.plugin-event-code {
  font-size: 0.75rem;
}
</style>
