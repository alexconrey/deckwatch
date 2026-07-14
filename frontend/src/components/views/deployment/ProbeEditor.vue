<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { ProbeConfig, ProbeInput, UpdateProbesRequest } from "@/types/api";

type ProbeKey = "liveness" | "readiness" | "startup";

interface ProbeFormRow {
  enabled: boolean;
  probe_type: "httpGet" | "tcpSocket" | "exec";
  path: string;
  port: number | null;
  command: string;
  initial_delay_seconds: number | null;
  period_seconds: number | null;
  timeout_seconds: number | null;
  failure_threshold: number | null;
  success_threshold: number | null;
}

const props = defineProps<{
  liveness: ProbeConfig | null;
  readiness: ProbeConfig | null;
  startup: ProbeConfig | null;
  loading?: boolean;
}>();

const emit = defineEmits<{
  save: [body: UpdateProbesRequest];
  cancel: [];
}>();

const emptyRow = (): ProbeFormRow => ({
  enabled: false,
  probe_type: "httpGet",
  path: "/",
  port: 80,
  command: "",
  initial_delay_seconds: null,
  period_seconds: null,
  timeout_seconds: null,
  failure_threshold: null,
  success_threshold: null,
});

const rowFromConfig = (cfg: ProbeConfig | null): ProbeFormRow => {
  if (!cfg) return emptyRow();
  const t = (cfg.probe_type === "httpGet" || cfg.probe_type === "tcpSocket" || cfg.probe_type === "exec")
    ? cfg.probe_type
    : "httpGet";
  return {
    enabled: true,
    probe_type: t,
    path: cfg.path ?? "/",
    port: cfg.port ?? 80,
    command: (cfg.command ?? []).join(" "),
    initial_delay_seconds: cfg.initial_delay_seconds,
    period_seconds: cfg.period_seconds,
    timeout_seconds: cfg.timeout_seconds,
    failure_threshold: cfg.failure_threshold,
    success_threshold: cfg.success_threshold,
  };
};

const forms = ref<Record<ProbeKey, ProbeFormRow>>({
  liveness: rowFromConfig(props.liveness),
  readiness: rowFromConfig(props.readiness),
  startup: rowFromConfig(props.startup),
});

watch(() => [props.liveness, props.readiness, props.startup], () => {
  forms.value = {
    liveness: rowFromConfig(props.liveness),
    readiness: rowFromConfig(props.readiness),
    startup: rowFromConfig(props.startup),
  };
}, { deep: true });

const rows: { key: ProbeKey; label: string; icon: string }[] = [
  { key: "liveness", label: "Liveness", icon: "mdi-heart-pulse" },
  { key: "readiness", label: "Readiness", icon: "mdi-check-network" },
  { key: "startup", label: "Startup", icon: "mdi-rocket-launch" },
];

const originals = computed(() => ({
  liveness: props.liveness,
  readiness: props.readiness,
  startup: props.startup,
}));

const toProbeInput = (r: ProbeFormRow): ProbeInput => {
  const base: ProbeInput = {
    probe_type: r.probe_type,
    initial_delay_seconds: r.initial_delay_seconds ?? undefined,
    period_seconds: r.period_seconds ?? undefined,
    timeout_seconds: r.timeout_seconds ?? undefined,
    failure_threshold: r.failure_threshold ?? undefined,
    success_threshold: r.success_threshold ?? undefined,
  };
  if (r.probe_type === "httpGet") {
    base.path = r.path || undefined;
    base.port = r.port ?? undefined;
  } else if (r.probe_type === "tcpSocket") {
    base.port = r.port ?? undefined;
  } else if (r.probe_type === "exec") {
    base.command = r.command.trim() ? r.command.trim().split(/\s+/) : undefined;
  }
  return base;
};

const handleSave = () => {
  const body: UpdateProbesRequest = {};
  for (const { key } of rows) {
    const orig = originals.value[key];
    const row = forms.value[key];
    const field = `${key}_probe` as keyof UpdateProbesRequest;
    if (row.enabled) {
      body[field] = toProbeInput(row);
    } else if (orig) {
      body[field] = null;
    }
  }
  emit("save", body);
};
</script>

<template>
  <div>
    <div v-for="row in rows" :key="row.key" class="mb-4">
      <div class="d-flex align-center mb-2">
        <v-icon :icon="row.icon" class="mr-2" size="small" />
        <span class="text-subtitle-2">{{ row.label }} Probe</span>
        <v-spacer />
        <v-switch
          v-model="forms[row.key].enabled"
          color="primary"
          hide-details
          density="compact"
          inset
        />
      </div>
      <div v-if="forms[row.key].enabled" class="pl-6">
        <v-row dense>
          <v-col cols="4">
            <v-select
              v-model="forms[row.key].probe_type"
              :items="['httpGet', 'tcpSocket', 'exec']"
              label="Type"
              density="compact"
              hide-details
            />
          </v-col>
          <v-col v-if="forms[row.key].probe_type === 'httpGet'" cols="5">
            <v-text-field
              v-model="forms[row.key].path"
              label="Path"
              placeholder="/healthz"
              density="compact"
              hide-details
            />
          </v-col>
          <v-col
            v-if="forms[row.key].probe_type === 'httpGet' || forms[row.key].probe_type === 'tcpSocket'"
            cols="3"
          >
            <v-text-field
              v-model.number="forms[row.key].port"
              label="Port"
              type="number"
              density="compact"
              hide-details
            />
          </v-col>
          <v-col v-if="forms[row.key].probe_type === 'exec'" cols="8">
            <v-text-field
              v-model="forms[row.key].command"
              label="Command (space-separated)"
              placeholder="cat /tmp/ready"
              density="compact"
              hide-details
            />
          </v-col>
        </v-row>
        <v-row dense class="mt-2">
          <v-col cols="6" md="3">
            <v-text-field
              v-model.number="forms[row.key].initial_delay_seconds"
              label="Initial Delay (s)"
              type="number"
              density="compact"
              hide-details
            />
          </v-col>
          <v-col cols="6" md="3">
            <v-text-field
              v-model.number="forms[row.key].period_seconds"
              label="Period (s)"
              type="number"
              density="compact"
              hide-details
            />
          </v-col>
          <v-col cols="6" md="3">
            <v-text-field
              v-model.number="forms[row.key].timeout_seconds"
              label="Timeout (s)"
              type="number"
              density="compact"
              hide-details
            />
          </v-col>
          <v-col cols="6" md="3">
            <v-text-field
              v-model.number="forms[row.key].failure_threshold"
              label="Failure Threshold"
              type="number"
              density="compact"
              hide-details
            />
          </v-col>
        </v-row>
      </div>
      <div v-else class="pl-6 text-caption text-secondary">
        {{ originals[row.key] ? "Will be removed on save" : "Disabled" }}
      </div>
      <v-divider class="mt-3" />
    </div>

    <div class="d-flex justify-end mt-4">
      <v-btn variant="text" @click="emit('cancel')">Cancel</v-btn>
      <v-btn color="primary" variant="flat" :loading="loading" @click="handleSave">
        Save
      </v-btn>
    </div>
  </div>
</template>
