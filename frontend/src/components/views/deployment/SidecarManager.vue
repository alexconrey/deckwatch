<script setup lang="ts">
import { computed, ref } from "vue";
import { deploymentsApi } from "@/api/deployments";
import type {
  AddContainerRequest,
  ContainerStatusSummary,
  DeploymentDetailResponse,
} from "@/types/api";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";

const props = defineProps<{
  namespace: string;
  deploymentName: string;
  containers: ContainerStatusSummary[];
}>();

const emit = defineEmits<{
  changed: [detail?: DeploymentDetailResponse];
}>();

const showForm = ref(false);
const saving = ref(false);
const error = ref<string | null>(null);
const confirmRemove = ref<string | null>(null);

const form = ref({
  name: "",
  image: "",
  port: null as number | null,
  command: "",
  args: "",
  env: "",
  cpuRequest: "",
  memRequest: "",
  cpuLimit: "",
  memLimit: "",
});

const primary = computed(() => props.containers[0]?.name ?? null);

const resetForm = () => {
  form.value = {
    name: "", image: "", port: null, command: "", args: "", env: "",
    cpuRequest: "", memRequest: "", cpuLimit: "", memLimit: "",
  };
};

const parseKvList = (raw: string) => {
  return raw.split(",").map((s) => s.trim()).filter(Boolean).map((pair) => {
    const eq = pair.indexOf("=");
    if (eq < 0) return { name: pair, value: "" };
    return { name: pair.slice(0, eq), value: pair.slice(eq + 1) };
  });
};

const parseArgs = (raw: string) => raw.trim().split(/\s+/).filter(Boolean);

const handleAdd = async () => {
  saving.value = true;
  error.value = null;
  try {
    const body: AddContainerRequest = {
      name: form.value.name.trim(),
      image: form.value.image.trim(),
    };
    if (form.value.port !== null) body.port = form.value.port;
    if (form.value.command.trim()) body.command = parseArgs(form.value.command);
    if (form.value.args.trim()) body.args = parseArgs(form.value.args);
    if (form.value.env.trim()) body.env = parseKvList(form.value.env);
    const reqCpu = form.value.cpuRequest.trim();
    const reqMem = form.value.memRequest.trim();
    if (reqCpu || reqMem) {
      body.resource_requests = { cpu: reqCpu || null, memory: reqMem || null };
    }
    const limCpu = form.value.cpuLimit.trim();
    const limMem = form.value.memLimit.trim();
    if (limCpu || limMem) {
      body.resource_limits = { cpu: limCpu || null, memory: limMem || null };
    }
    const detail = await deploymentsApi.addContainer(props.namespace, props.deploymentName, body);
    emit("changed", detail);
    showForm.value = false;
    resetForm();
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to add sidecar";
  } finally {
    saving.value = false;
  }
};

const handleRemove = async () => {
  if (!confirmRemove.value) return;
  saving.value = true;
  error.value = null;
  try {
    const detail = await deploymentsApi.removeContainer(
      props.namespace, props.deploymentName, confirmRemove.value,
    );
    emit("changed", detail);
    confirmRemove.value = null;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to remove sidecar";
  } finally {
    saving.value = false;
  }
};
</script>

<template>
  <v-card class="mb-4">
    <v-card-title class="d-flex align-center">
      <v-icon icon="mdi-view-grid-plus" class="mr-2" size="small" />
      <span class="text-subtitle-1">Containers</span>
      <v-spacer />
      <v-btn size="small" variant="text" prepend-icon="mdi-plus" @click="showForm = true">
        Add Sidecar
      </v-btn>
    </v-card-title>

    <v-alert v-if="error" type="error" density="compact" class="mx-4 mb-2" closable>
      {{ error }}
    </v-alert>

    <v-list density="compact">
      <v-list-item v-for="(c, idx) in containers" :key="c.name" :title="c.name" :subtitle="c.image">
        <template #prepend>
          <v-icon :icon="idx === 0 ? 'mdi-star' : 'mdi-cube-outline'" size="small" :color="idx === 0 ? 'primary' : undefined" />
        </template>
        <template #append>
          <v-chip v-if="idx === 0" size="x-small" color="primary" variant="tonal">primary</v-chip>
          <v-btn v-else size="small" variant="text" color="error" icon="mdi-close" @click="confirmRemove = c.name" />
        </template>
      </v-list-item>
      <v-list-item v-if="containers.length === 0">
        <v-list-item-title class="text-secondary text-body-2 text-center">
          No containers reported yet
        </v-list-item-title>
      </v-list-item>
    </v-list>

    <v-dialog v-model="showForm" max-width="640">
      <v-card>
        <v-card-title>Add Sidecar Container</v-card-title>
        <v-card-text>
          <v-row dense>
            <v-col cols="6">
              <v-text-field v-model="form.name" label="Name" placeholder="my-sidecar" density="compact" />
            </v-col>
            <v-col cols="6">
              <v-text-field v-model.number="form.port" label="Port (optional)" type="number" density="compact" />
            </v-col>
          </v-row>
          <v-text-field v-model="form.image" label="Image" placeholder="nginx:1.27-alpine" density="compact" />
          <v-text-field v-model="form.command" label="Command (space-separated, optional)" placeholder="/bin/sh" density="compact" />
          <v-text-field v-model="form.args" label="Args (space-separated, optional)" placeholder="-c 'sleep infinity'" density="compact" />
          <v-text-field v-model="form.env" label="Env (KEY=val, comma-separated)" placeholder="LOG_LEVEL=info,MODE=proxy" density="compact" />
          <v-divider class="my-3" />
          <div class="text-subtitle-2 mb-2">Resources (optional)</div>
          <v-row dense>
            <v-col cols="6"><v-text-field v-model="form.cpuRequest" label="CPU Request" placeholder="100m" density="compact" /></v-col>
            <v-col cols="6"><v-text-field v-model="form.memRequest" label="Memory Request" placeholder="128Mi" density="compact" /></v-col>
            <v-col cols="6"><v-text-field v-model="form.cpuLimit" label="CPU Limit" placeholder="500m" density="compact" /></v-col>
            <v-col cols="6"><v-text-field v-model="form.memLimit" label="Memory Limit" placeholder="256Mi" density="compact" /></v-col>
          </v-row>
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="showForm = false">Cancel</v-btn>
          <v-btn color="primary" variant="flat" :loading="saving" :disabled="!form.name.trim() || !form.image.trim()" @click="handleAdd">
            Add
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <ConfirmDialog
      :model-value="confirmRemove !== null"
      title="Remove Sidecar"
      :message="`Remove container '${confirmRemove}' from this deployment?`"
      confirm-text="Remove"
      :loading="saving"
      @update:model-value="(v: boolean) => { if (!v) confirmRemove = null }"
      @confirm="handleRemove"
    />
  </v-card>
</template>
