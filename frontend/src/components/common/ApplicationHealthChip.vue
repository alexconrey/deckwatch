<script setup lang="ts">
import { computed } from "vue";
import type { ApplicationHealth } from "@/types/api";

const props = defineProps<{ health: ApplicationHealth }>();

const chipConfig = computed(() => {
  switch (props.health) {
    case "healthy":
      return { color: "success", icon: "mdi-check-circle", text: "Healthy" };
    case "degraded":
      return { color: "warning", icon: "mdi-alert", text: "Degraded" };
    case "unhealthy":
      return { color: "error", icon: "mdi-close-circle", text: "Unhealthy" };
    case "empty":
      return { color: "grey", icon: "mdi-package-variant-closed", text: "Empty" };
    default:
      return { color: "secondary", icon: "mdi-help-circle", text: "Unknown" };
  }
});
</script>

<template>
  <v-chip :color="chipConfig.color" size="small" variant="flat">
    <v-icon :icon="chipConfig.icon" start size="small" />
    {{ chipConfig.text }}
  </v-chip>
</template>
