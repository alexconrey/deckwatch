<script setup lang="ts">
import { computed } from "vue";
import type { PluginHealth } from "@/types/api";

// A badge for plugin health. `label` prefixes the text, e.g. "Plugins".
const props = withDefaults(
  defineProps<{
    health: PluginHealth;
    label?: string;
    size?: "x-small" | "small" | "default";
  }>(),
  { label: "", size: "small" },
);

const look = computed(() => {
  switch (props.health) {
    case "failing":
      return { color: "error", icon: "mdi-alert-circle", text: "failing" };
    case "degraded":
      return { color: "warning", icon: "mdi-alert", text: "degraded" };
    default:
      return { color: "success", icon: "mdi-check-circle", text: "healthy" };
  }
});
</script>

<template>
  <v-chip
    :color="look.color"
    :prepend-icon="look.icon"
    :size="size"
    variant="tonal"
    :data-health="health"
    data-testid="plugin-health-chip"
  >
    {{ label ? `${label}: ${look.text}` : look.text }}
  </v-chip>
</template>
