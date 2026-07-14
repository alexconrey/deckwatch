<script setup lang="ts">
import { computed } from "vue";

const props = withDefaults(
  defineProps<{
    data: number[];
    label?: string;
    unit?: string;
    color?: string;
    height?: number;
    width?: number;
    lineWidth?: number;
    smooth?: number | boolean;
    fill?: boolean;
    latestFormatted?: string;
    percentOfLimit?: number | null;
  }>(),
  {
    label: "",
    unit: "",
    color: "primary",
    height: 48,
    width: 96,
    lineWidth: 2,
    smooth: 4,
    fill: false,
    latestFormatted: "",
    percentOfLimit: null,
  },
);

const effectiveColor = computed(() => {
  if (props.percentOfLimit == null) return props.color;
  if (props.percentOfLimit >= 85) return "error";
  if (props.percentOfLimit >= 60) return "warning";
  return "success";
});

const displayData = computed<number[]>(() => {
  if (props.data.length === 0) return [];
  if (props.data.length === 1) return [0, props.data[0]];
  return props.data;
});

const hasData = computed(() => props.data.length > 0);
</script>

<template>
  <div class="metric-sparkline d-inline-flex align-center ga-2">
    <div v-if="label" class="text-caption text-secondary">{{ label }}</div>
    <div v-if="!hasData" class="text-caption text-disabled" :style="{ width: width + 'px' }">
      &mdash;
    </div>
    <v-sparkline
      v-else
      :model-value="displayData"
      :color="effectiveColor"
      :line-width="lineWidth"
      :smooth="smooth"
      :fill="fill"
      :height="height"
      :width="width"
      auto-draw
      padding="2"
    />
    <div v-if="latestFormatted" class="text-caption font-weight-medium">
      {{ latestFormatted }}<span v-if="unit" class="text-secondary">{{ unit }}</span>
    </div>
  </div>
</template>
