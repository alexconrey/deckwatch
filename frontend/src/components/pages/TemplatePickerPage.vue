<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { templatesApi } from "@/api/templates";
import type { DeploymentTemplate } from "@/types/api";

const router = useRouter();
const templates = ref<DeploymentTemplate[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);

onMounted(async () => {
  try {
    const res = await templatesApi.list();
    templates.value = res.templates;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to load templates";
  } finally {
    loading.value = false;
  }
});

const chooseTemplate = (t: DeploymentTemplate) => {
  // We stash the payload in sessionStorage rather than encoding it in the
  // query string — the payload includes probes/resources and quickly runs
  // past sane URL length limits. The CreateDeploymentPage reads and clears
  // this key on mount.
  sessionStorage.setItem("deckwatch.template.payload", JSON.stringify(t.payload));
  sessionStorage.setItem("deckwatch.template.id", t.id);
  router.push({ name: "CreateDeployment", query: { template: t.id } });
};

const startBlank = () => {
  sessionStorage.removeItem("deckwatch.template.payload");
  sessionStorage.removeItem("deckwatch.template.id");
  router.push({ name: "CreateDeployment" });
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
      <h2 class="text-h5 ml-2">Choose a Template</h2>
      <v-spacer />
      <v-btn variant="outlined" size="small" @click="startBlank">
        Start Blank
      </v-btn>
    </div>

    <v-alert v-if="error" type="error" class="mb-4" closable>
      {{ error }}
    </v-alert>

    <v-progress-linear v-if="loading" indeterminate color="primary" />

    <v-row v-else>
      <v-col
        v-for="t in templates"
        :key="t.id"
        cols="12"
        sm="6"
        md="4"
        lg="3"
      >
        <v-card
          class="pa-4 h-100 d-flex flex-column"
          variant="outlined"
          hover
          @click="chooseTemplate(t)"
        >
          <div class="d-flex align-center mb-2">
            <v-icon :icon="t.icon" size="32" color="primary" class="mr-2" />
            <span class="text-h6">{{ t.name }}</span>
          </div>
          <div class="text-body-2 text-secondary flex-grow-1">
            {{ t.description }}
          </div>
          <v-btn
            variant="tonal"
            color="primary"
            class="mt-3"
            append-icon="mdi-arrow-right"
            @click.stop="chooseTemplate(t)"
          >
            Use Template
          </v-btn>
        </v-card>
      </v-col>
    </v-row>
  </div>
</template>
