<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useNamespaceStore } from "@/stores/namespace";
import { deploymentsApi } from "@/api/deployments";
import { deploymentsUxApi } from "@/api/deployments_additions";
import type { CreateDeploymentRequest } from "@/types/api";
import DeploymentForm from "@/components/views/deployment/DeploymentForm.vue";

const route = useRoute();
const router = useRouter();
const ns = useNamespaceStore();
const loading = ref(false);
const error = ref<string | null>(null);
const templateInitial = ref<Partial<CreateDeploymentRequest> | undefined>();
const templateId = ref<string | null>(null);

onMounted(() => {
  // If the user arrived from the template picker, hydrate the form with the
  // stashed payload. We accept either the ?template=<id> query flag OR a
  // bare presence of the sessionStorage key — the latter handles the case
  // where the router redirected without preserving query params.
  const stashed = sessionStorage.getItem("deckwatch.template.payload");
  if (route.query.template || stashed) {
    if (stashed) {
      try {
        templateInitial.value = JSON.parse(stashed) as Partial<CreateDeploymentRequest>;
      } catch {
        // corrupted stash; ignore rather than block the form
      }
    }
    templateId.value = sessionStorage.getItem("deckwatch.template.id");
    // One-shot: clear so a browser refresh doesn't repopulate stale values.
    sessionStorage.removeItem("deckwatch.template.payload");
    sessionStorage.removeItem("deckwatch.template.id");
  }
});

const validate = async (values: CreateDeploymentRequest) => {
  if (!ns.selected) return { ok: false, errors: ["No namespace selected"] };
  return deploymentsUxApi.validate(ns.selected, values);
};

const handleSubmit = async (values: CreateDeploymentRequest) => {
  if (!ns.selected) return;
  loading.value = true;
  error.value = null;
  try {
    await deploymentsApi.create(ns.selected, values);
    router.push({
      name: "DeploymentDetail",
      params: { namespace: ns.selected, name: values.name },
    });
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to create deployment";
  } finally {
    loading.value = false;
  }
};
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <v-btn
        icon="mdi-arrow-left"
        variant="text"
        @click="router.push({ name: 'Deployments' })"
      />
      <h2 class="text-h5 ml-2">Create Deployment</h2>
      <v-spacer />
      <v-btn
        variant="outlined"
        size="small"
        prepend-icon="mdi-view-grid-outline"
        @click="router.push({ name: 'TemplatePicker' })"
      >
        Templates
      </v-btn>
    </div>

    <v-alert v-if="!ns.selected" type="info" class="mb-4">
      Select a namespace first.
    </v-alert>

    <v-alert v-if="templateId" type="success" density="compact" class="mb-4">
      Pre-filled from template: <strong>{{ templateId }}</strong>
    </v-alert>

    <v-alert v-if="error" type="error" class="mb-4" closable>
      {{ error }}
    </v-alert>

    <v-card v-if="ns.selected" class="pa-4">
      <v-chip class="mb-4" color="primary" variant="outlined" size="small">
        {{ ns.selected }}
      </v-chip>
      <DeploymentForm
        :initial-values="templateInitial"
        :loading="loading"
        :on-validate="validate"
        @submit="handleSubmit"
      />
    </v-card>
  </div>
</template>
