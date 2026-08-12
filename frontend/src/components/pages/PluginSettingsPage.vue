<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { pluginsApi } from "@/api/plugins";
import { settingsApi } from "@/api/settings";
import { useSnackbar } from "@/composables/useSnackbar";
import type { ConfigField, PluginConfig, PluginSummary } from "@/types/api";

const route = useRoute();
const router = useRouter();
const { success, error: showError } = useSnackbar();

const pluginName = computed(() => route.params.name as string);

// --- State ---
const loading = ref(true);
const saving = ref(false);
const plugin = ref<PluginSummary | null>(null);
const schema = ref<ConfigField[]>([]);

// Flat map of key → current form value.
const formValues = ref<Record<string, string>>({});

// Visibility toggle for password fields (key → show/hide).
const showPassword = ref<Record<string, boolean>>({});

// The plugin config entry loaded from settings (for reading saved values).
const savedPluginConfig = ref<PluginConfig | null>(null);

// --- Load ---
async function load() {
  loading.value = true;
  try {
    const [allPlugins, settings] = await Promise.all([
      pluginsApi.list(),
      settingsApi.get(),
    ]);

    const found = allPlugins.find((p) => p.name === pluginName.value) ?? null;
    plugin.value = found;
    schema.value = found?.config_schema ?? [];

    // Find matching PluginConfig entry in settings for pre-population.
    savedPluginConfig.value =
      (settings.plugins ?? []).find((p) => p.name === pluginName.value) ?? null;

    initFormValues();
  } catch (e) {
    showError(e instanceof Error ? e.message : "Failed to load plugin");
  } finally {
    loading.value = false;
  }
}

function initFormValues() {
  const saved = savedPluginConfig.value?.config ?? {};
  const next: Record<string, string> = {};
  const nextShow: Record<string, boolean> = {};

  for (const field of schema.value) {
    if (field.field_type === "secret") {
      // If the backend masked the value as "configured", start with an empty
      // input so the operator must actively retype to overwrite. The
      // placeholder text communicates the existing state.
      const savedVal = saved[field.key] ?? "";
      next[field.key] = savedVal === "configured" ? "" : savedVal;
      nextShow[field.key] = false;
    } else {
      next[field.key] = saved[field.key] ?? field.default ?? "";
    }
  }

  formValues.value = next;
  showPassword.value = nextShow;
}

onMounted(load);

// --- Helpers ---

/** True when this secret field already has a stored (encrypted) value. */
function isSecretConfigured(field: ConfigField): boolean {
  const saved = savedPluginConfig.value?.config ?? {};
  return saved[field.key] === "configured";
}

/** True when the field's value is inherited from an env var in inherit_env_keys. */
function isFromEnv(field: ConfigField): boolean {
  return (
    !!field.env_source &&
    (savedPluginConfig.value?.inherit_env_keys ?? []).includes(field.key)
  );
}

/** True when the field's value is inherited from a file path in inherit_env_file_keys. */
function isFromFile(field: ConfigField): boolean {
  return (
    !!field.env_source &&
    field.key in (savedPluginConfig.value?.inherit_env_file_keys ?? {})
  );
}

/** Human-readable badge label for an env_source field. */
function envSourceLabel(field: ConfigField): string {
  if (isFromFile(field)) {
    const fileVar =
      (savedPluginConfig.value?.inherit_env_file_keys ?? {})[field.key];
    return `From file: ${fileVar ?? field.env_source}`;
  }
  if (isFromEnv(field)) {
    return `From env: ${field.env_source}`;
  }
  // Declared but not yet wired up in inherit_env_keys.
  return `From env: ${field.env_source} (not yet configured)`;
}

/** Chip colour for the env_source badge. */
function envSourceColor(field: ConfigField): string {
  return isFromEnv(field) || isFromFile(field) ? "info" : "warning";
}

/** Validation rule for required non-env fields. */
function requiredRule(field: ConfigField) {
  return (v: string) =>
    !field.required || !!field.env_source || !!v || `${field.label} is required`;
}

// --- Save ---
async function save() {
  saving.value = true;
  try {
    // Include only editable (non-env_source) fields.
    // For secret fields, skip when empty so we don't overwrite an existing
    // encrypted value with a blank string.
    const payload: Record<string, string> = {};
    for (const field of schema.value) {
      if (field.env_source) continue;
      const val = formValues.value[field.key] ?? "";
      if (field.field_type === "secret" && !val) continue;
      payload[field.key] = val;
    }

    await pluginsApi.saveConfig(pluginName.value, payload);

    // Trigger settings reload so the backend picks up the new config on its
    // next plugin invocation cycle.
    const current = await settingsApi.get();
    await settingsApi.update(current);

    success("Plugin configuration saved");
    // Re-fetch so secret "Configured" chips reflect the new state.
    await load();
  } catch (e) {
    showError(e instanceof Error ? e.message : "Failed to save configuration");
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div>
    <!-- Header -->
    <div class="d-flex align-center mb-4">
      <v-btn
        icon="mdi-arrow-left"
        variant="text"
        @click="router.push({ name: 'Settings' })"
      />
      <div class="ml-2">
        <h2 class="text-h5">{{ pluginName }}</h2>
        <span class="text-caption text-secondary">Plugin configuration</span>
      </div>
      <v-spacer />
      <v-btn
        color="primary"
        prepend-icon="mdi-content-save"
        :loading="saving"
        :disabled="loading || !plugin"
        @click="save"
      >
        Save
      </v-btn>
    </div>

    <!-- Loading -->
    <v-progress-linear v-if="loading" indeterminate color="primary" class="mb-4" />

    <!-- Plugin not found / not loaded -->
    <v-alert
      v-else-if="!plugin"
      type="warning"
      variant="tonal"
      class="mb-4"
    >
      Plugin <strong>{{ pluginName }}</strong> is not currently loaded. Verify
      it is configured and enabled in Settings &rarr; Plugins.
    </v-alert>

    <template v-else>
      <!-- Plugin info card -->
      <v-card variant="outlined" class="mb-6 pa-4">
        <div class="d-flex align-center ga-3">
          <v-icon icon="mdi-puzzle" color="primary" size="large" />
          <div class="flex-grow-1">
            <div class="text-subtitle-1 font-weight-medium">{{ plugin.name }}</div>
            <div class="text-caption text-secondary">
              {{ plugin.description || "No description provided." }}
            </div>
          </div>
          <div class="text-right">
            <v-chip size="small" color="success" variant="tonal">Loaded</v-chip>
            <div class="text-caption text-secondary mt-1">v{{ plugin.version }}</div>
          </div>
        </div>
      </v-card>

      <!-- No schema -->
      <v-alert
        v-if="schema.length === 0"
        type="info"
        variant="tonal"
        class="mb-4"
      >
        This plugin does not declare any configuration fields.
      </v-alert>

      <!-- Dynamic form -->
      <v-form v-else @submit.prevent="save">
        <div v-for="field in schema" :key="field.key" class="mb-6">

          <!-- env_source: read-only with badge -->
          <template v-if="field.env_source">
            <v-text-field
              :model-value="formValues[field.key]"
              :label="field.label"
              :hint="field.description"
              persistent-hint
              variant="outlined"
              density="comfortable"
              readonly
            >
              <template #append-inner>
                <v-chip
                  :color="envSourceColor(field)"
                  size="x-small"
                  variant="tonal"
                  :prepend-icon="
                    isFromEnv(field) || isFromFile(field)
                      ? 'mdi-link-variant'
                      : 'mdi-alert-outline'
                  "
                >
                  {{ envSourceLabel(field) }}
                </v-chip>
              </template>
            </v-text-field>
          </template>

          <!-- bool: switch -->
          <template v-else-if="field.field_type === 'bool'">
            <v-switch
              v-model="formValues[field.key]"
              true-value="true"
              false-value="false"
              :label="field.label"
              :hint="field.description"
              persistent-hint
              color="primary"
              density="comfortable"
              hide-details="auto"
            />
          </template>

          <!-- select: dropdown -->
          <template v-else-if="field.field_type === 'select'">
            <v-select
              v-model="formValues[field.key]"
              :items="field.options"
              :label="field.label"
              :hint="field.description"
              persistent-hint
              variant="outlined"
              density="comfortable"
              :rules="[requiredRule(field)]"
            />
          </template>

          <!-- secret: password with eye toggle -->
          <template v-else-if="field.field_type === 'secret'">
            <v-text-field
              v-model="formValues[field.key]"
              :label="field.label"
              :hint="field.description"
              persistent-hint
              variant="outlined"
              density="comfortable"
              :type="showPassword[field.key] ? 'text' : 'password'"
              :placeholder="isSecretConfigured(field) ? 'already configured' : ''"
              :rules="[requiredRule(field)]"
              autocomplete="new-password"
            >
              <template #prepend-inner>
                <v-chip
                  v-if="isSecretConfigured(field)"
                  size="x-small"
                  color="success"
                  variant="tonal"
                  prepend-icon="mdi-lock-check"
                  class="mr-1"
                >
                  Configured
                </v-chip>
              </template>
              <template #append-inner>
                <v-btn
                  :icon="showPassword[field.key] ? 'mdi-eye-off' : 'mdi-eye'"
                  variant="text"
                  size="small"
                  density="compact"
                  @click="showPassword[field.key] = !showPassword[field.key]"
                />
              </template>
            </v-text-field>
          </template>

          <!-- string: plain text -->
          <template v-else>
            <v-text-field
              v-model="formValues[field.key]"
              :label="field.label"
              :hint="field.description"
              persistent-hint
              variant="outlined"
              density="comfortable"
              :rules="[requiredRule(field)]"
            />
          </template>
        </div>

        <div class="d-flex justify-end mt-4">
          <v-btn
            color="primary"
            variant="flat"
            prepend-icon="mdi-content-save"
            :loading="saving"
            type="submit"
          >
            Save configuration
          </v-btn>
        </div>
      </v-form>
    </template>
  </div>
</template>
