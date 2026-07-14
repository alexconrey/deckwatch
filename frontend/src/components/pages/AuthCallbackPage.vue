<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { settingsApi } from "@/api/settings";
import { useAuth } from "@/composables/useAuth";

const router = useRouter();
const auth = useAuth();
const error = ref<string | null>(null);

onMounted(async () => {
  try {
    // A callback page can be loaded cold (e.g. Entra redirects into a fresh
    // browser tab) so the composable might not have auth settings loaded
    // yet. Fetch them before verifying the token — nonce/state validation
    // still works regardless, but downstream `isEnabled()` checks need it.
    try {
      const s = await settingsApi.get();
      auth.setAuthSettings(s.auth ?? null);
    } catch {
      /* non-fatal */
    }

    const { returnTo } = await auth.handleCallback();
    // replace() so the fragment-carrying URL doesn't sit in browser history.
    await router.replace(returnTo || "/");
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
});
</script>

<template>
  <v-container class="d-flex flex-column align-center justify-center" style="min-height: 60vh">
    <template v-if="error">
      <v-icon icon="mdi-alert-circle" color="error" size="64" class="mb-4" />
      <h2 class="text-h5 mb-2">Sign-in failed</h2>
      <p class="text-body-2 text-secondary text-center mb-4" style="max-width: 480px">
        {{ error }}
      </p>
      <v-btn color="primary" @click="auth.login()">Try again</v-btn>
    </template>
    <template v-else>
      <v-progress-circular indeterminate color="primary" size="48" class="mb-4" />
      <p class="text-body-1 text-secondary">Signing you in…</p>
    </template>
  </v-container>
</template>
