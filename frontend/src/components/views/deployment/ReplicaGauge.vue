<script setup lang="ts">
import { computed } from "vue";
import type { ReplicaCounts } from "@/types/api";

const props = defineProps<{ replicas: ReplicaCounts }>();

const segments = computed(() => {
  const total = Math.max(props.replicas.desired, 1);
  const ready = props.replicas.available;
  return { ready, total };
});

const percentage = computed(() => {
  if (segments.value.total === 0) return 100;
  return Math.round((segments.value.ready / segments.value.total) * 100);
});

const color = computed(() => {
  if (percentage.value >= 100) return "success";
  if (percentage.value > 0) return "warning";
  return "error";
});
</script>

<template>
  <div class="d-flex align-center ga-2">
    <v-progress-linear
      :model-value="percentage"
      :color="color"
      bg-color="surface-variant"
      height="8"
      rounded
      style="max-width: 120px"
    />
    <span class="text-body-2 text-secondary">
      {{ replicas.available }}/{{ replicas.desired }}
    </span>
  </div>
</template>
