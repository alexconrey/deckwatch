<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { addonsApi } from "@/api/addons";
import { deploymentsApi } from "@/api/deployments";
import type {
  AddonDefinition,
  ContainerStatusSummary,
  UpdateAddonRequest,
} from "@/types/api";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import AddonEditDialog from "./AddonEditDialog.vue";

const props = defineProps<{
  namespace: string;
  deploymentName: string;
  containers?: ContainerStatusSummary[];
}>();

const emit = defineEmits<{ changed: [] }>();

interface AttachedAddon {
  addonId: string;
  containerName: string;
  injectedEnv: string[];
}

const catalog = ref<AddonDefinition[]>([]);
const attached = ref<AttachedAddon[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const showCatalog = ref(false);
const confirmDetach = ref<AttachedAddon | null>(null);
const attaching = ref<string | null>(null);
const editing = ref<AttachedAddon | null>(null);
const saving = ref(false);

const ADDON_ANNOTATION_PREFIX = "deckwatch.addon/";
// Backend records the env-var names it injected into the primary container
// for each addon under `deckwatch.addon-env/<container>` as a comma-separated
// list. Surfacing them here so users can see what env vars the addon exposes.
const ADDON_ENV_ANNOTATION_PREFIX = "deckwatch.addon-env/";

const loadCatalog = async () => {
  try {
    const res = await addonsApi.list();
    catalog.value = res.addons;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to load addon catalog";
  }
};

const loadAttached = async () => {
  try {
    const detail = await deploymentsApi.get(props.namespace, props.deploymentName);
    const anns = detail.annotations ?? {};
    attached.value = Object.entries(anns)
      .filter(([k]) => k.startsWith(ADDON_ANNOTATION_PREFIX))
      .map(([k, v]) => {
        const containerName = k.slice(ADDON_ANNOTATION_PREFIX.length);
        const envRaw = anns[`${ADDON_ENV_ANNOTATION_PREFIX}${containerName}`] ?? "";
        const injectedEnv = envRaw
          .split(",")
          .map((s) => s.trim())
          .filter((s) => s.length > 0);
        return { addonId: v, containerName, injectedEnv };
      });
  } catch {
    attached.value = [];
  }
};

onMounted(async () => {
  await Promise.all([loadCatalog(), loadAttached()]);
});

const attachedIds = computed(() => new Set(attached.value.map((a) => a.addonId)));

const handleAttach = async (addon: AddonDefinition) => {
  attaching.value = addon.id;
  error.value = null;
  try {
    await addonsApi.attach(props.namespace, props.deploymentName, addon.id);
    await loadAttached();
    emit("changed");
    showCatalog.value = false;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to attach addon";
  } finally {
    attaching.value = null;
  }
};

const handleDetach = async () => {
  if (!confirmDetach.value) return;
  loading.value = true;
  error.value = null;
  try {
    await addonsApi.detach(
      props.namespace,
      props.deploymentName,
      confirmDetach.value.addonId,
    );
    confirmDetach.value = null;
    await loadAttached();
    emit("changed");
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to detach addon";
  } finally {
    loading.value = false;
  }
};

const handleSaveEdit = async (body: UpdateAddonRequest) => {
  if (!editing.value) return;
  saving.value = true;
  error.value = null;
  try {
    await addonsApi.updateAddon(
      props.namespace,
      props.deploymentName,
      editing.value.addonId,
      body,
    );
    editing.value = null;
    await loadAttached();
    emit("changed");
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to update addon";
  } finally {
    saving.value = false;
  }
};

const defFor = (id: string) => catalog.value.find((a) => a.id === id);
const editingDef = computed(() => (editing.value ? defFor(editing.value.addonId) ?? null : null));

const ENV_TOOLTIP =
  "This environment variable is automatically injected into your primary container when this addon is attached.";
</script>

<template>
  <v-card class="mb-4">
    <v-card-title class="d-flex align-center">
      <v-icon icon="mdi-puzzle" class="mr-2" size="small" />
      <span class="text-subtitle-1">Addons</span>
      <v-spacer />
      <v-btn size="small" variant="text" prepend-icon="mdi-plus" @click="showCatalog = true">
        Add Addon
      </v-btn>
    </v-card-title>

    <v-alert v-if="error" type="error" density="compact" class="mx-4 mb-2" closable>
      {{ error }}
    </v-alert>

    <div v-if="attached.length === 0" class="text-center py-4 text-secondary text-body-2">
      No addons attached
    </div>

    <v-list v-else density="compact">
      <v-list-item
        v-for="a in attached"
        :key="a.containerName"
        :title="defFor(a.addonId)?.name ?? a.addonId"
        :subtitle="`container: ${a.containerName}`"
      >
        <template #prepend>
          <v-icon icon="mdi-cube-outline" size="small" />
        </template>
        <template #append>
          <v-btn
            size="small"
            variant="text"
            icon="mdi-pencil"
            :title="'Edit addon configuration'"
            @click="editing = a"
          />
          <v-btn size="small" variant="text" color="error" icon="mdi-close" @click="confirmDetach = a" />
        </template>
        <template v-if="a.injectedEnv.length > 0" #default>
          <div class="mt-1 d-flex flex-wrap ga-1">
            <template v-for="name in a.injectedEnv" :key="name">
              <v-tooltip location="top" open-delay="200">
                <template #activator="{ props: tipProps }">
                  <v-chip
                    v-bind="tipProps"
                    size="x-small"
                    variant="tonal"
                    color="primary"
                  >
                    {{ name }}
                  </v-chip>
                </template>
                <span>{{ ENV_TOOLTIP }}</span>
              </v-tooltip>
            </template>
          </div>
        </template>
      </v-list-item>
    </v-list>

    <v-dialog v-model="showCatalog" max-width="640">
      <v-card>
        <v-card-title>Available Addons</v-card-title>
        <v-card-text>
          <v-list>
            <v-list-item v-for="addon in catalog" :key="addon.id" :title="addon.name" :subtitle="addon.description">
              <template #append>
                <v-btn
                  size="small" color="primary" variant="flat"
                  :loading="attaching === addon.id"
                  :disabled="attachedIds.has(addon.id)"
                  @click="handleAttach(addon)"
                >
                  {{ attachedIds.has(addon.id) ? "Attached" : "Add" }}
                </v-btn>
              </template>
            </v-list-item>
          </v-list>
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="showCatalog = false">Close</v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <AddonEditDialog
      :model-value="editing !== null"
      :addon="editingDef"
      :container-name="editing?.containerName ?? ''"
      :current-port="null"
      :current-env="[]"
      :current-cpu-request="null"
      :current-mem-request="null"
      :current-cpu-limit="null"
      :current-mem-limit="null"
      :loading="saving"
      @update:model-value="(v: boolean) => { if (!v) editing = null }"
      @save="handleSaveEdit"
    />

    <ConfirmDialog
      :model-value="confirmDetach !== null"
      title="Remove Addon"
      :message="`Remove addon container '${confirmDetach?.containerName}' from this deployment?`"
      confirm-text="Remove"
      :loading="loading"
      @update:model-value="(v: boolean) => { if (!v) confirmDetach = null }"
      @confirm="handleDetach"
    />
  </v-card>
</template>
