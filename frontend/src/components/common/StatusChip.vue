<script setup lang="ts">
import { computed } from "vue";
import type { DeploymentPhase } from "@/types/api";

const props = defineProps<{ status: DeploymentPhase }>();

const emit = defineEmits<{
  click: [];
}>();

const chipConfig = computed(() => {
  switch (props.status) {
    case "available":
      return { color: "success", icon: "mdi-check-circle", text: "Available" };
    case "progressing":
      return { color: "info", icon: "mdi-progress-clock", text: "Progressing" };
    case "degraded":
      return { color: "warning", icon: "mdi-alert", text: "Degraded" };
    case "failed":
      return { color: "error", icon: "mdi-close-circle", text: "Failed" };
    default:
      return { color: "secondary", icon: "mdi-help-circle", text: "Unknown" };
  }
});

const clickable = computed(
  () => props.status === "failed" || props.status === "degraded",
);
</script>

<template>
  <v-chip
    :color="chipConfig.color"
    size="small"
    variant="flat"
    :style="clickable ? { cursor: 'pointer' } : undefined"
    @click="clickable && emit('click')"
  >
    <v-icon :icon="chipConfig.icon" start size="small" />
    {{ chipConfig.text }}
    <v-icon
      v-if="clickable"
      icon="mdi-information-outline"
      end
      size="x-small"
    />
  </v-chip>
</template>
