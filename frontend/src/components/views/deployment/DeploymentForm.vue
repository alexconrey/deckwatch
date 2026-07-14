<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import type {
  ContainerPortInput,
  CostSettings,
  CreateDeploymentRequest,
  ResourceSpec,
} from "@/types/api";
import { settingsApi } from "@/api/settings";
import RegistryTagPicker from "@/components/common/RegistryTagPicker.vue";
import {
  estimateCost,
  formatCost,
  isCostIncreaseOverFactor,
  type HourlyCost,
} from "@/utils/cost";

const props = defineProps<{
  initialValues?: Partial<CreateDeploymentRequest>;
  isEdit?: boolean;
  loading?: boolean;
  /** When set, a "Validate" button appears that invokes this callback with
   *  the current form state. The callback returns { ok, errors } and the
   *  form renders the result inline. Omit to hide the button (e.g. on the
   *  edit-existing flow where a dry-run against the mutation path is less
   *  useful). */
  onValidate?: (values: CreateDeploymentRequest) => Promise<{
    ok: boolean;
    errors: string[];
  }>;
}>();

const emit = defineEmits<{
  submit: [values: CreateDeploymentRequest];
}>();

interface ProbeForm {
  enabled: boolean;
  probeType: string;
  path: string;
  port: number;
  command: string;
  initialDelay: number | undefined;
  period: number | undefined;
  timeout: number | undefined;
  failureThreshold: number | undefined;
  successThreshold: number | undefined;
}

interface PortForm {
  port: number | undefined;
  name: string;
  protocol: string;
}

const defaultProbe = (): ProbeForm => ({
  enabled: false,
  probeType: "httpGet",
  path: "/",
  port: 80,
  command: "",
  initialDelay: undefined,
  period: undefined,
  timeout: undefined,
  failureThreshold: undefined,
  successThreshold: undefined,
});

const defaultPort = (): PortForm => ({
  port: undefined,
  name: "",
  protocol: "TCP",
});

const form = ref({
  name: "",
  image: "",
  replicas: 1,
  ports: [] as PortForm[],
  env: [] as { name: string; value: string }[],
  command: "",
  args: "",
  cpuRequest: "",
  memoryRequest: "",
  cpuLimit: "",
  memoryLimit: "",
  livenessProbe: defaultProbe(),
  readinessProbe: defaultProbe(),
  startupProbe: defaultProbe(),
});

const registryPickerOpen = ref(false);

// Snapshot of the resource requests and replica count as the form was
// first opened, used to warn when an edit substantially raises cost.
// Populated once from initialValues in edit mode; a null baseline (no prior
// requests) suppresses the warning since there's no defensible ratio to
// compare against.
const baseline = ref<{
  requests: ResourceSpec | null;
  replicas: number;
} | null>(null);

// Loaded from settings; null when the cluster hasn't configured cost rates.
const costSettings = ref<CostSettings | null>(null);

watch(
  () => props.initialValues,
  (v) => {
    if (v) {
      form.value.name = v.name ?? "";
      form.value.image = v.image ?? "";
      form.value.replicas = v.replicas ?? 1;
      // Prefer the newer `ports` array, but fall back to the legacy single
      // `port` field so templates and older API responses still populate the
      // form correctly.
      if (v.ports && v.ports.length > 0) {
        form.value.ports = v.ports.map((p) => ({
          port: p.port,
          name: p.name ?? "",
          protocol: (p.protocol ?? "TCP").toUpperCase(),
        }));
      } else if (typeof v.port === "number") {
        form.value.ports = [{ port: v.port, name: "", protocol: "TCP" }];
      } else {
        form.value.ports = [];
      }
      form.value.env = v.env ?? [];
      form.value.command = v.command?.join(", ") ?? "";
      form.value.args = v.args?.join(", ") ?? "";
      form.value.cpuRequest = v.resource_requests?.cpu ?? "";
      form.value.memoryRequest = v.resource_requests?.memory ?? "";
      form.value.cpuLimit = v.resource_limits?.cpu ?? "";
      form.value.memoryLimit = v.resource_limits?.memory ?? "";
      if (v.liveness_probe) {
        form.value.livenessProbe = probeToForm(v.liveness_probe);
      }
      if (v.readiness_probe) {
        form.value.readinessProbe = probeToForm(v.readiness_probe);
      }
      if (v.startup_probe) {
        form.value.startupProbe = probeToForm(v.startup_probe);
      }
      // Capture the initial state as the cost baseline only on the edit
      // flow. Skipping this on create means the ">2x" warning never fires
      // there — expected, since a new deployment has no "before" cost.
      if (props.isEdit && baseline.value === null) {
        const hasReqs = !!(v.resource_requests?.cpu || v.resource_requests?.memory);
        baseline.value = {
          requests: hasReqs
            ? {
                cpu: v.resource_requests?.cpu ?? null,
                memory: v.resource_requests?.memory ?? null,
              }
            : null,
          replicas: v.replicas ?? 1,
        };
      }
    }
  },
  { immediate: true },
);

// Cluster-wide default resource requests/limits, loaded from the Settings
// page. Only applied when the operator has explicitly configured them — an
// unconfigured cluster leaves the fields blank rather than injecting a
// silent default. On the edit-existing flow we skip this entirely so the
// user always sees the live spec, not a settings-derived overlay. The cost
// overlay, however, applies to both create and edit modes.
onMounted(async () => {
  try {
    const settings = await settingsApi.get();
    costSettings.value = settings.cost ?? null;
    if (props.isEdit) return;
    const d = settings.default_resource_limits;
    if (!d) return;
    // Only fill fields that are still blank — template payloads and initial
    // values win over the cluster defaults.
    if (!form.value.cpuRequest && d.cpu_request) {
      form.value.cpuRequest = d.cpu_request;
    }
    if (!form.value.memoryRequest && d.memory_request) {
      form.value.memoryRequest = d.memory_request;
    }
    if (!form.value.cpuLimit && d.cpu_limit) {
      form.value.cpuLimit = d.cpu_limit;
    }
    if (!form.value.memoryLimit && d.memory_limit) {
      form.value.memoryLimit = d.memory_limit;
    }
  } catch {
    // Settings are optional context; a failure here should not block the form.
  }
});

function probeToForm(p: {
  probe_type: string;
  path?: string | null;
  port?: number | null;
  command?: string[] | null;
  initial_delay_seconds?: number | null;
  period_seconds?: number | null;
  timeout_seconds?: number | null;
  failure_threshold?: number | null;
  success_threshold?: number | null;
}): ProbeForm {
  return {
    enabled: true,
    probeType: p.probe_type,
    path: p.path ?? "/",
    port: p.port ?? 80,
    command: p.command?.join(", ") ?? "",
    initialDelay: p.initial_delay_seconds ?? undefined,
    period: p.period_seconds ?? undefined,
    timeout: p.timeout_seconds ?? undefined,
    failureThreshold: p.failure_threshold ?? undefined,
    successThreshold: p.success_threshold ?? undefined,
  };
}

const addPort = () => {
  form.value.ports.push(defaultPort());
};

const removePort = (index: number) => {
  form.value.ports.splice(index, 1);
};

const addEnvVar = () => {
  form.value.env.push({ name: "", value: "" });
};

const removeEnvVar = (index: number) => {
  form.value.env.splice(index, 1);
};

function formToProbeInput(pf: ProbeForm): CreateDeploymentRequest["liveness_probe"] {
  if (!pf.enabled) return undefined;
  const input: NonNullable<CreateDeploymentRequest["liveness_probe"]> = {
    probe_type: pf.probeType,
  };
  if (pf.probeType === "httpGet") {
    input.path = pf.path;
    input.port = pf.port;
  } else if (pf.probeType === "tcpSocket") {
    input.port = pf.port;
  } else if (pf.probeType === "exec") {
    input.command = pf.command
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
  }
  if (pf.initialDelay) input.initial_delay_seconds = pf.initialDelay;
  if (pf.period) input.period_seconds = pf.period;
  if (pf.timeout) input.timeout_seconds = pf.timeout;
  if (pf.failureThreshold) input.failure_threshold = pf.failureThreshold;
  if (pf.successThreshold) input.success_threshold = pf.successThreshold;
  return input;
}

const buildRequest = (): CreateDeploymentRequest => {
  const req: CreateDeploymentRequest = {
    name: form.value.name,
    image: form.value.image,
    replicas: form.value.replicas,
  };

  const validPorts: ContainerPortInput[] = form.value.ports
    .filter((p) => typeof p.port === "number" && p.port > 0)
    .map((p) => {
      const entry: ContainerPortInput = { port: p.port as number };
      if (p.name.trim()) entry.name = p.name.trim();
      // Only send `protocol` when it differs from the k8s default (TCP) so we
      // don't churn deployment specs that were previously created without it.
      if (p.protocol && p.protocol.toUpperCase() !== "TCP") {
        entry.protocol = p.protocol.toUpperCase();
      }
      return entry;
    });
  if (validPorts.length > 0) req.ports = validPorts;
  if (form.value.env.length > 0) {
    req.env = form.value.env.filter((e) => e.name);
  }
  if (form.value.command) {
    req.command = form.value.command
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
  }
  if (form.value.args) {
    req.args = form.value.args
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
  }
  if (form.value.cpuRequest || form.value.memoryRequest) {
    req.resource_requests = {
      cpu: form.value.cpuRequest || null,
      memory: form.value.memoryRequest || null,
    };
  }
  if (form.value.cpuLimit || form.value.memoryLimit) {
    req.resource_limits = {
      cpu: form.value.cpuLimit || null,
      memory: form.value.memoryLimit || null,
    };
  }

  const lp = formToProbeInput(form.value.livenessProbe);
  const rp = formToProbeInput(form.value.readinessProbe);
  const sp = formToProbeInput(form.value.startupProbe);
  if (lp) req.liveness_probe = lp;
  if (rp) req.readiness_probe = rp;
  if (sp) req.startup_probe = sp;

  return req;
};

const handleSubmit = () => {
  emit("submit", buildRequest());
};

const onImagePicked = (ref: string) => {
  form.value.image = ref;
};

// ---- Cost estimation ----

const currentRequests = computed<ResourceSpec | null>(() => {
  if (!form.value.cpuRequest && !form.value.memoryRequest) return null;
  return {
    cpu: form.value.cpuRequest || null,
    memory: form.value.memoryRequest || null,
  };
});

const currentCost = computed<HourlyCost | null>(() =>
  estimateCost(currentRequests.value, form.value.replicas, costSettings.value),
);

const baselineCost = computed<HourlyCost | null>(() => {
  if (!baseline.value) return null;
  return estimateCost(
    baseline.value.requests,
    baseline.value.replicas,
    costSettings.value,
  );
});

const showCostChip = computed(() => currentCost.value !== null);

const costOverBudget = computed(() =>
  isCostIncreaseOverFactor(baselineCost.value, currentCost.value, 2),
);

const costChipText = computed(() => {
  const c = currentCost.value;
  if (!c) return "";
  return `~${formatCost(c.hourly, c.currency)}/hr · ~${formatCost(c.monthly, c.currency)}/mo`;
});

const baselineChipText = computed(() => {
  const c = baselineCost.value;
  if (!c) return "";
  return `Current: ~${formatCost(c.hourly, c.currency)}/hr`;
});

// ---- Validation state ----
const validating = ref(false);
const validationResult = ref<{ ok: boolean; errors: string[] } | null>(null);

const handleValidate = async () => {
  if (!props.onValidate) return;
  validating.value = true;
  validationResult.value = null;
  try {
    validationResult.value = await props.onValidate(buildRequest());
  } catch (e) {
    // Surface unexpected errors as a single validation failure so the user
    // isn't left staring at a spinner.
    validationResult.value = {
      ok: false,
      errors: [e instanceof Error ? e.message : "Validation request failed"],
    };
  } finally {
    validating.value = false;
  }
};

const probeTypes = [
  { title: "HTTP GET", value: "httpGet" },
  { title: "TCP Socket", value: "tcpSocket" },
  { title: "Exec", value: "exec" },
];

const protocolOptions = [
  { title: "TCP", value: "TCP" },
  { title: "UDP", value: "UDP" },
];

const REPLICAS_MAX = 20;

const clampReplicas = (n: number | undefined | null): number => {
  const v = typeof n === "number" && Number.isFinite(n) ? Math.round(n) : 0;
  if (v < 0) return 0;
  if (v > REPLICAS_MAX) return REPLICAS_MAX;
  return v;
};

const onReplicasInput = (v: unknown) => {
  const parsed =
    typeof v === "number" ? v : typeof v === "string" ? Number(v) : NaN;
  form.value.replicas = clampReplicas(Number.isNaN(parsed) ? 0 : parsed);
};
</script>

<template>
  <v-form @submit.prevent="handleSubmit">
    <v-row>
      <v-col cols="12" md="6">
        <v-text-field
          v-model="form.name"
          label="Deployment Name"
          :disabled="isEdit"
          :rules="[(v: string) => !!v || 'Required']"
          required
        />
      </v-col>
      <v-col cols="12" md="6">
        <v-text-field
          v-model="form.image"
          label="Container Image"
          placeholder="nginx:latest"
          :rules="[(v: string) => !!v || 'Required']"
          required
        >
          <template #append-inner>
            <v-tooltip location="top" text="Browse the deckwatch registry">
              <template #activator="{ props: tipProps }">
                <v-btn
                  v-bind="tipProps"
                  icon="mdi-image-search"
                  size="small"
                  variant="text"
                  density="comfortable"
                  @click.stop="registryPickerOpen = true"
                />
              </template>
            </v-tooltip>
          </template>
        </v-text-field>
      </v-col>
    </v-row>

    <div class="text-subtitle-2 mb-2">Replicas</div>
    <v-row dense align="center" class="mb-2">
      <v-col cols="12" md="9">
        <v-slider
          :model-value="form.replicas"
          :min="0"
          :max="REPLICAS_MAX"
          :step="1"
          thumb-label
          color="primary"
          hide-details
          density="compact"
          @update:model-value="onReplicasInput"
        />
      </v-col>
      <v-col cols="12" md="3">
        <v-text-field
          :model-value="form.replicas"
          label="Replicas"
          type="number"
          min="0"
          :max="REPLICAS_MAX"
          density="compact"
          hide-details
          @update:model-value="onReplicasInput"
        />
      </v-col>
    </v-row>

    <v-divider class="my-4" />
    <div class="text-subtitle-2 mb-2">Container Ports</div>
    <div class="text-caption text-secondary mb-2">
      Optional. Add one row per port the container exposes. Name is used to
      reference the port from Services and Ingresses.
    </div>

    <v-row v-for="(portRow, i) in form.ports" :key="i" dense align="center">
      <v-col cols="4" md="3">
        <v-text-field
          v-model.number="portRow.port"
          label="Port"
          type="number"
          min="1"
          max="65535"
          density="compact"
          hide-details
          :rules="[
            (v: unknown) =>
              (typeof v === 'number' && v >= 1 && v <= 65535) ||
              'Port must be 1–65535',
          ]"
        />
      </v-col>
      <v-col cols="5" md="4">
        <v-text-field
          v-model="portRow.name"
          label="Name (optional)"
          placeholder="http"
          density="compact"
          hide-details
        />
      </v-col>
      <v-col cols="2" md="3">
        <v-select
          v-model="portRow.protocol"
          :items="protocolOptions"
          label="Protocol"
          density="compact"
          hide-details
        />
      </v-col>
      <v-col cols="1" md="2" class="text-right">
        <v-btn
          icon="mdi-close"
          size="small"
          variant="text"
          @click="removePort(i)"
        />
      </v-col>
    </v-row>

    <v-btn
      variant="text"
      size="small"
      prepend-icon="mdi-plus"
      class="mb-4"
      @click="addPort"
    >
      Add Port
    </v-btn>

    <v-divider class="my-4" />
    <div class="text-subtitle-2 mb-2">Environment Variables</div>

    <v-row v-for="(env, i) in form.env" :key="i" dense>
      <v-col cols="5">
        <v-text-field
          v-model="env.name"
          label="Name"
          density="compact"
          hide-details
        />
      </v-col>
      <v-col cols="5">
        <v-text-field
          v-model="env.value"
          label="Value"
          density="compact"
          hide-details
        />
      </v-col>
      <v-col cols="2">
        <v-btn
          icon="mdi-close"
          size="small"
          variant="text"
          @click="removeEnvVar(i)"
        />
      </v-col>
    </v-row>

    <v-btn
      variant="text"
      size="small"
      prepend-icon="mdi-plus"
      class="mb-4"
      @click="addEnvVar"
    >
      Add Variable
    </v-btn>

    <v-divider class="my-4" />
    <div class="text-subtitle-2 mb-2">Resources</div>
    <div class="text-caption text-secondary mb-2">
      Optional. Leave blank to run without explicit requests or limits.
      Cluster-wide defaults from the Settings page are applied here when
      configured.
    </div>

    <v-row dense>
      <v-col cols="6" md="3">
        <v-text-field
          v-model="form.cpuRequest"
          label="CPU Request"
          placeholder="e.g. 100m"
          density="compact"
        />
      </v-col>
      <v-col cols="6" md="3">
        <v-text-field
          v-model="form.memoryRequest"
          label="Memory Request"
          placeholder="e.g. 128Mi"
          density="compact"
        />
      </v-col>
      <v-col cols="6" md="3">
        <v-text-field
          v-model="form.cpuLimit"
          label="CPU Limit"
          placeholder="e.g. 500m"
          density="compact"
        />
      </v-col>
      <v-col cols="6" md="3">
        <v-text-field
          v-model="form.memoryLimit"
          label="Memory Limit"
          placeholder="e.g. 256Mi"
          density="compact"
        />
      </v-col>
    </v-row>

    <!-- Cost estimate: only rendered when the cluster has a cost overlay
         configured AND the current form has at least one priced request. -->
    <div v-if="showCostChip" class="d-flex align-center flex-wrap ga-2 mt-2">
      <v-chip
        size="small"
        variant="tonal"
        :color="costOverBudget ? 'warning' : 'primary'"
        prepend-icon="mdi-currency-usd"
      >
        {{ costChipText }}
      </v-chip>
      <v-chip
        v-if="baselineCost"
        size="small"
        variant="outlined"
      >
        {{ baselineChipText }}
      </v-chip>
      <span class="text-caption text-secondary">
        Estimate based on requests × replicas; excludes egress, storage, and control-plane costs.
      </span>
    </div>
    <v-alert
      v-if="costOverBudget"
      type="warning"
      variant="tonal"
      density="compact"
      class="mt-3"
      icon="mdi-alert-circle-outline"
    >
      <div class="font-weight-medium">
        This edit more than doubles the current cost.
      </div>
      <div class="text-body-2">
        {{ baselineChipText }} → {{ costChipText }}. Confirm the change is intentional before submitting.
      </div>
    </v-alert>

    <v-divider class="my-4" />
    <div class="text-subtitle-2 mb-2">Health Checks</div>

    <template
      v-for="(probeKey, probeLabel) in {
        livenessProbe: 'Liveness Probe',
        readinessProbe: 'Readiness Probe',
        startupProbe: 'Startup Probe',
      } as Record<string, string>"
      :key="probeKey"
    >
      <v-card variant="outlined" class="pa-3 mb-3">
        <v-switch
          v-model="(form as any)[probeKey].enabled"
          :label="probeLabel"
          color="primary"
          hide-details
          density="compact"
        />
        <template v-if="(form as any)[probeKey].enabled">
          <v-row dense class="mt-2">
            <v-col cols="12" md="3">
              <v-select
                v-model="(form as any)[probeKey].probeType"
                :items="probeTypes"
                label="Type"
                density="compact"
              />
            </v-col>
            <v-col
              v-if="(form as any)[probeKey].probeType === 'httpGet'"
              cols="12"
              md="5"
            >
              <v-text-field
                v-model="(form as any)[probeKey].path"
                label="Path"
                placeholder="/"
                density="compact"
              />
            </v-col>
            <v-col
              v-if="
                (form as any)[probeKey].probeType === 'httpGet' ||
                (form as any)[probeKey].probeType === 'tcpSocket'
              "
              cols="12"
              md="2"
            >
              <v-text-field
                v-model.number="(form as any)[probeKey].port"
                label="Port"
                type="number"
                density="compact"
              />
            </v-col>
            <v-col
              v-if="(form as any)[probeKey].probeType === 'exec'"
              cols="12"
              md="7"
            >
              <v-text-field
                v-model="(form as any)[probeKey].command"
                label="Command (comma-separated)"
                placeholder="cat, /tmp/healthy"
                density="compact"
              />
            </v-col>
          </v-row>
          <v-row dense>
            <v-col cols="6" md="2">
              <v-text-field
                v-model.number="(form as any)[probeKey].initialDelay"
                label="Delay (s)"
                type="number"
                density="compact"
                hide-details
              />
            </v-col>
            <v-col cols="6" md="2">
              <v-text-field
                v-model.number="(form as any)[probeKey].period"
                label="Period (s)"
                type="number"
                density="compact"
                hide-details
              />
            </v-col>
            <v-col cols="6" md="2">
              <v-text-field
                v-model.number="(form as any)[probeKey].timeout"
                label="Timeout (s)"
                type="number"
                density="compact"
                hide-details
              />
            </v-col>
            <v-col cols="6" md="3">
              <v-text-field
                v-model.number="(form as any)[probeKey].failureThreshold"
                label="Failure Threshold"
                type="number"
                density="compact"
                hide-details
              />
            </v-col>
            <v-col cols="6" md="3">
              <v-text-field
                v-model.number="(form as any)[probeKey].successThreshold"
                label="Success Threshold"
                type="number"
                density="compact"
                hide-details
              />
            </v-col>
          </v-row>
        </template>
      </v-card>
    </template>

    <v-divider class="my-4" />
    <div class="text-subtitle-2 mb-2">Advanced</div>

    <v-row dense>
      <v-col cols="12" md="6">
        <v-text-field
          v-model="form.command"
          label="Command (comma-separated)"
          placeholder="/bin/sh, -c"
          density="compact"
        />
      </v-col>
      <v-col cols="12" md="6">
        <v-text-field
          v-model="form.args"
          label="Args (comma-separated)"
          placeholder="--config, /etc/config.yaml"
          density="compact"
        />
      </v-col>
    </v-row>

    <!-- Validation result banner -->
    <v-alert
      v-if="validationResult && validationResult.ok"
      type="success"
      density="compact"
      class="mt-4"
      closable
      @click:close="validationResult = null"
    >
      Dry-run passed — the API server accepted this spec.
    </v-alert>
    <v-alert
      v-else-if="validationResult && !validationResult.ok"
      type="error"
      density="compact"
      class="mt-4"
      closable
      @click:close="validationResult = null"
    >
      <div class="font-weight-medium mb-1">Validation failed:</div>
      <ul class="pl-4">
        <li
          v-for="(msg, i) in validationResult.errors"
          :key="i"
          class="text-body-2"
        >
          {{ msg }}
        </li>
      </ul>
    </v-alert>

    <div class="d-flex justify-end mt-4 ga-2">
      <v-btn
        v-if="onValidate"
        variant="outlined"
        size="large"
        prepend-icon="mdi-check-decagram"
        :loading="validating"
        :disabled="loading"
        @click="handleValidate"
      >
        Validate
      </v-btn>
      <v-btn
        type="submit"
        color="primary"
        variant="flat"
        :loading="loading"
        size="large"
      >
        {{ isEdit ? "Update Deployment" : "Create Deployment" }}
      </v-btn>
    </div>

    <RegistryTagPicker
      v-model="registryPickerOpen"
      @selected="onImagePicked"
    />
  </v-form>
</template>
