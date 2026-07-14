<script setup lang="ts">
import { computed } from "vue";

const props = defineProps<{
  phase: string;
  ready: boolean;
}>();

const iconConfig = computed(() => {
  if (props.phase === "Running" && props.ready)
    return { icon: "mdi-check-circle", color: "success" };
  if (props.phase === "Running" && !props.ready)
    return { icon: "mdi-alert-circle", color: "warning" };
  if (props.phase === "Pending")
    return { icon: "mdi-clock-outline", color: "info" };
  if (props.phase === "Succeeded")
    return { icon: "mdi-check-circle-outline", color: "success" };
  if (props.phase === "Failed")
    return { icon: "mdi-close-circle", color: "error" };
  return { icon: "mdi-help-circle-outline", color: "secondary" };
});
</script>

<template>
  <v-icon :icon="iconConfig.icon" :color="iconConfig.color" size="small" />
</template>
