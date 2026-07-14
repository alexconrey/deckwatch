<script setup lang="ts">
import { useRouter } from "vue-router";

const router = useRouter();
</script>

<template>
  <div class="d-flex flex-column align-center justify-center" style="height: 60vh">
    <v-icon icon="mdi-compass-off-outline" size="64" color="secondary" class="mb-4" />
    <h2 class="text-h5 mb-2">Page Not Found</h2>
    <p class="text-body-1 text-secondary mb-4">
      The page you're looking for doesn't exist.
    </p>
    <v-btn color="primary" @click="router.push({ name: 'Deployments' })">
      Go to Deployments
    </v-btn>
  </div>
</template>
