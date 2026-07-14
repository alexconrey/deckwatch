<template>
  <v-app>
    <router-view />
    <v-snackbar
      v-model="snackbar.show"
      :color="snackbar.color"
      :timeout="snackbar.timeout"
      location="top right"
    >
      {{ snackbar.message }}
      <template #actions>
        <v-btn variant="text" @click="snackbar.show = false">Close</v-btn>
      </template>
    </v-snackbar>
    <AlertToastStack />
  </v-app>
</template>

<script setup lang="ts">
import AlertToastStack from "@/components/common/AlertToastStack.vue";
import { useMetrics } from "@/composables/useMetrics";
import { useSnackbar } from "@/composables/useSnackbar";

// Wire up page-view tracking, error hooks, and periodic flush at the root.
useMetrics();

// Global toast surface -- any component can trigger via useSnackbar().
const { snackbar } = useSnackbar();
</script>
