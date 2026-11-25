<script setup lang="ts">
// The application's Observability section: the Prometheus PodMonitor of
// each of its Deployments (the same toggle as on the deployment page), and
// OpenTelemetry injection into its Deployments, StatefulSets and CronJobs.
// GET/PUT /api/namespaces/{ns}/applications/{name}/observability; the
// reconciler applies saved injection settings within a few seconds.
import { computed, ref, watch } from "vue";
import { applicationsApi } from "@/api/applications";
import { monitoringApi } from "@/api/monitoring";
import { usePolling } from "@/composables/usePolling";
import { useSnackbar } from "@/composables/useSnackbar";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import MonitorConfigDialog from "@/components/views/deployment/MonitorConfigDialog.vue";
import type {
  ApplicationObservability,
  DeploymentMonitor,
  OtelInjectionSettings,
  OtlpSignal,
} from "@/types/api";
import type { MonitorConfigRequest } from "@/types/monitoring";
import { SIGNAL_LABELS } from "@/utils/observability";
import {
  type ConfirmCopy,
  confirmCopy,
  formatResourceAttributes,
  injectionChange,
  injectionStateColor,
  injectionStateLabel,
  parseResourceAttributes,
  validateServiceName,
} from "@/utils/appObservability";

const props = defineProps<{
  namespace: string;
  name: string;
}>();

const { success, error: snackError } = useSnackbar();

const obs = ref<ApplicationObservability | null>(null);
const loadError = ref<string | null>(null);

async function fetchObservability() {
  try {
    obs.value = await applicationsApi.getObservability(props.namespace, props.name);
    loadError.value = null;
  } catch (e) {
    loadError.value = e instanceof Error ? e.message : "Failed to load the observability settings";
  }
}

usePolling(fetchObservability, 10000);

// ---- Prometheus monitoring ------------------------------------------------

const prometheus = computed(() => obs.value?.prometheus ?? null);
const monitorTarget = ref<DeploymentMonitor | null>(null);
const showMonitorDialog = ref(false);
const showDisableMonitor = ref(false);
const monitorBusy = ref(false);

function editMonitor(m: DeploymentMonitor) {
  monitorTarget.value = m;
  showMonitorDialog.value = true;
}

function askDisableMonitor(m: DeploymentMonitor) {
  monitorTarget.value = m;
  showDisableMonitor.value = true;
}

async function saveMonitor(body: MonitorConfigRequest) {
  if (!monitorTarget.value) return;
  monitorBusy.value = true;
  try {
    await monitoringApi.upsert(props.namespace, monitorTarget.value.deployment, body);
    showMonitorDialog.value = false;
    await fetchObservability();
  } catch (e) {
    snackError(e instanceof Error ? e.message : "Failed to save the monitor");
  } finally {
    monitorBusy.value = false;
  }
}

async function disableMonitor() {
  if (!monitorTarget.value) return;
  monitorBusy.value = true;
  try {
    await monitoringApi.disable(props.namespace, monitorTarget.value.deployment);
    showDisableMonitor.value = false;
    await fetchObservability();
  } catch (e) {
    snackError(e instanceof Error ? e.message : "Failed to disable the monitor");
  } finally {
    monitorBusy.value = false;
  }
}

// ---- OpenTelemetry --------------------------------------------------------

const otel = computed(() => obs.value?.opentelemetry ?? null);

interface Form {
  enabled: boolean;
  serviceName: string;
  attributes: string;
}

const toForm = (s: OtelInjectionSettings): Form => ({
  enabled: s.enabled,
  serviceName: s.service_name ?? "",
  attributes: formatResourceAttributes(s.resource_attributes),
});

const form = ref<Form>({ enabled: false, serviceName: "", attributes: "" });
// What the form was last filled from: a poll refreshes the form only while
// it still shows that, so it never clobbers an edit in progress.
const formBase = ref<Form | null>(null);

const sameForm = (a: Form, b: Form) =>
  a.enabled === b.enabled && a.serviceName === b.serviceName && a.attributes === b.attributes;

watch(
  otel,
  (current) => {
    if (!current) return;
    const fresh = toForm(current);
    if (!formBase.value || sameForm(form.value, formBase.value)) {
      form.value = { ...fresh };
    }
    formBase.value = fresh;
  },
  { immediate: true },
);

const serviceNameError = computed(() => validateServiceName(form.value.serviceName));
const parsedAttributes = computed(() => parseResourceAttributes(form.value.attributes));

const stored = computed<OtelInjectionSettings>(() => ({
  enabled: otel.value?.enabled ?? false,
  service_name: otel.value?.service_name ?? null,
  resource_attributes: otel.value?.resource_attributes ?? [],
}));

const requested = computed<OtelInjectionSettings>(() => ({
  enabled: form.value.enabled,
  service_name: form.value.serviceName.trim() || null,
  resource_attributes: parsedAttributes.value.attributes,
}));

const change = computed(() => injectionChange(stored.value, requested.value));
const canSave = computed(
  () =>
    otel.value !== null &&
    change.value !== null &&
    serviceNameError.value === null &&
    parsedAttributes.value.error === null,
);

const hasEndpoint = computed(
  () => !!otel.value && (otel.value.otlp_endpoint !== null || otel.value.otlp_metrics_endpoint !== null),
);

const saving = ref(false);
const saveError = ref<string | null>(null);
const confirm = ref<ConfirmCopy | null>(null);
const showConfirm = ref(false);

function resetForm() {
  if (otel.value) form.value = toForm(otel.value);
  saveError.value = null;
}

function requestSave() {
  if (!canSave.value || !change.value || !otel.value) return;
  const copy = confirmCopy(change.value, requested.value.enabled, otel.value.workloads, hasEndpoint.value);
  if (copy === null) {
    void save();
    return;
  }
  confirm.value = copy;
  showConfirm.value = true;
}

async function save() {
  saving.value = true;
  saveError.value = null;
  try {
    obs.value = await applicationsApi.updateObservability(props.namespace, props.name, {
      opentelemetry: requested.value,
    });
    form.value = toForm(obs.value.opentelemetry);
    formBase.value = toForm(obs.value.opentelemetry);
    showConfirm.value = false;
    success("OpenTelemetry settings saved");
  } catch (e) {
    showConfirm.value = false;
    saveError.value = e instanceof Error ? e.message : "Failed to save the OpenTelemetry settings";
  } finally {
    saving.value = false;
  }
}

const exporterVar = (signal: OtlpSignal) => `OTEL_${signal.toUpperCase()}_EXPORTER=none`;
</script>

<template>
  <div data-testid="app-observability">
    <v-alert v-if="loadError" type="error" variant="tonal" density="compact" class="mb-4">
      {{ loadError }}
    </v-alert>
    <v-progress-linear v-if="!obs && !loadError" indeterminate color="primary" class="mb-4" />

    <!-- Prometheus monitoring -->
    <v-card v-if="prometheus" class="mb-4" data-testid="app-prometheus">
      <v-card-title class="d-flex align-center">
        <v-icon icon="mdi-chart-line" class="mr-2" size="small" />
        <span class="text-subtitle-1">Prometheus monitoring</span>
      </v-card-title>
      <v-card-text>
        <p class="text-body-2 text-secondary mb-3">
          A PodMonitor per Deployment, for the prometheus-operator to scrape. It's the same
          setting as the Prometheus Monitor on each deployment's page.
        </p>
        <v-alert
          v-if="!prometheus.available"
          type="warning"
          variant="tonal"
          density="compact"
          data-testid="app-prometheus-unavailable"
        >
          {{ prometheus.unavailable_reason }}
        </v-alert>
        <v-table v-else density="compact">
          <thead>
            <tr>
              <th>Deployment</th>
              <th>Scraping</th>
              <th>Endpoint</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="m in prometheus.deployments"
              :key="m.deployment"
              :data-testid="`monitor-${m.deployment}`"
            >
              <td class="font-weight-medium">{{ m.deployment }}</td>
              <td>
                <v-chip size="x-small" variant="tonal" :color="m.enabled ? 'success' : undefined">
                  {{ m.enabled ? "On" : "Off" }}
                </v-chip>
              </td>
              <td>
                <span v-if="m.enabled" class="text-caption">
                  Port <code>{{ m.port }}</code>, <code>{{ m.path }}</code>, every
                  {{ m.interval }}
                </span>
                <span v-else class="text-caption text-secondary">—</span>
              </td>
              <td class="text-right">
                <template v-if="m.enabled">
                  <v-btn size="small" variant="text" prepend-icon="mdi-pencil" @click="editMonitor(m)">
                    Edit
                  </v-btn>
                  <v-btn
                    size="small"
                    variant="text"
                    color="error"
                    prepend-icon="mdi-close"
                    @click="askDisableMonitor(m)"
                  >
                    Disable
                  </v-btn>
                </template>
                <v-btn v-else size="small" variant="text" prepend-icon="mdi-plus" @click="editMonitor(m)">
                  Enable
                </v-btn>
              </td>
            </tr>
            <tr v-if="prometheus.deployments.length === 0">
              <td colspan="4" class="text-center text-secondary text-body-2 py-4">
                No deployments in this application.
              </td>
            </tr>
          </tbody>
        </v-table>
      </v-card-text>
    </v-card>

    <!-- OpenTelemetry -->
    <v-card v-if="otel" data-testid="app-otel">
      <v-card-title class="d-flex align-center">
        <v-icon icon="mdi-chart-timeline-variant" class="mr-2" size="small" />
        <span class="text-subtitle-1">OpenTelemetry</span>
        <v-spacer />
        <v-chip size="small" variant="tonal" :color="otel.enabled ? 'success' : undefined">
          {{ otel.enabled ? "Injecting" : "Off" }}
        </v-chip>
      </v-card-title>
      <v-card-text>
        <p class="text-body-2 text-secondary mb-3">
          deckwatch sets the standard <code>OTEL_*</code> variables on the primary container of
          this application's Deployments, StatefulSets and CronJobs, so their OpenTelemetry SDK
          exports to the endpoints in Settings → Observability. Variables a workload sets in its
          <code>env</code> are kept.
        </p>

        <v-alert
          v-if="!hasEndpoint"
          type="warning"
          variant="tonal"
          density="compact"
          class="mb-3"
          data-testid="app-otel-no-endpoint"
        >
          No OTLP endpoint is set in
          <router-link :to="{ name: 'Settings' }">Settings → Observability</router-link>,
          so nothing is injected.
        </v-alert>
        <div v-else class="mb-3" data-testid="app-otel-signals">
          <div v-for="s in otel.signals" :key="s.signal" class="text-body-2">
            <span class="text-secondary mr-1">{{ SIGNAL_LABELS[s.signal] }}:</span>
            <code v-if="s.endpoint">{{ s.endpoint }}</code>
            <span v-else class="text-secondary">
              turned off (<code>{{ exporterVar(s.signal) }}</code>)
            </span>
          </div>
        </div>

        <v-switch
          v-model="form.enabled"
          label="Inject OpenTelemetry variables"
          color="primary"
          density="compact"
          hide-details
          class="mb-3"
          data-testid="app-otel-enabled"
        />
        <v-text-field
          v-model="form.serviceName"
          label="Service name"
          placeholder="Each workload's name"
          persistent-placeholder
          hint="OTEL_SERVICE_NAME for every workload. Leave empty to use each workload's own name."
          persistent-hint
          :error-messages="serviceNameError ?? []"
          variant="outlined"
          density="compact"
          class="mb-3"
          data-testid="app-otel-service-name"
        />
        <v-text-field
          v-model="form.attributes"
          label="Extra resource attributes"
          placeholder="deployment.environment=prod, team=payments"
          persistent-placeholder
          hint="key=value pairs, added to OTEL_RESOURCE_ATTRIBUTES after k8s.namespace.name, k8s.<kind>.name and deckwatch.application.name. Percent-encode values: a space is %20."
          persistent-hint
          :error-messages="parsedAttributes.error ?? []"
          variant="outlined"
          density="compact"
          data-testid="app-otel-attributes"
        />
        <v-alert
          v-if="saveError"
          type="error"
          variant="tonal"
          density="compact"
          class="mt-3"
          data-testid="app-otel-save-error"
        >
          {{ saveError }}
        </v-alert>
      </v-card-text>
      <v-card-actions>
        <v-spacer />
        <v-btn variant="text" :disabled="change === null || saving" @click="resetForm">Reset</v-btn>
        <v-btn
          color="primary"
          variant="flat"
          :disabled="!canSave"
          :loading="saving && !showConfirm"
          @click="requestSave"
        >
          Save
        </v-btn>
      </v-card-actions>

      <v-divider />
      <v-table density="compact" data-testid="app-otel-workloads">
        <thead>
          <tr>
            <th>Workload</th>
            <th>Container</th>
            <th>Injection</th>
            <th>Kept from the workload</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="w in otel.workloads" :key="`${w.kind}/${w.name}`">
            <td>
              <span class="text-caption text-secondary mr-1">{{ w.kind }}</span>
              <span class="font-weight-medium">{{ w.name }}</span>
            </td>
            <td>
              <code v-if="w.container">{{ w.container }}</code>
            </td>
            <td>
              <v-chip size="x-small" variant="tonal" :color="injectionStateColor(w.state)">
                {{ injectionStateLabel(w, otel.enabled) }}
              </v-chip>
            </td>
            <td>
              <span v-if="w.kept.length" class="text-caption">
                <code v-for="k in w.kept" :key="k" class="mr-1">{{ k }}</code>
              </span>
              <span v-if="w.exports_itself" class="text-caption text-secondary d-block">
                Its own export setup: deckwatch only sets the service name and attributes.
              </span>
              <span v-if="!w.kept.length && !w.exports_itself" class="text-caption text-secondary">
                —
              </span>
            </td>
          </tr>
          <tr v-if="otel.workloads.length === 0">
            <td colspan="4" class="text-center text-secondary text-body-2 py-4">
              No Deployments, StatefulSets or CronJobs in this application.
            </td>
          </tr>
        </tbody>
      </v-table>
      <v-alert
        v-if="otel.workloads_error"
        type="warning"
        variant="tonal"
        density="compact"
        class="ma-3"
      >
        {{ otel.workloads_error }}
      </v-alert>
    </v-card>

    <MonitorConfigDialog
      v-model="showMonitorDialog"
      :namespace="namespace"
      :enabled="monitorTarget?.enabled ?? false"
      :current="monitorTarget"
      :loading="monitorBusy"
      @save="saveMonitor"
    />
    <ConfirmDialog
      v-model="showDisableMonitor"
      title="Disable Prometheus Monitor"
      message="This will delete the PodMonitor for this deployment. Prometheus will stop scraping the workload."
      confirm-text="Disable"
      :loading="monitorBusy"
      @confirm="disableMonitor"
    />
    <ConfirmDialog
      v-model="showConfirm"
      :title="confirm?.title ?? ''"
      :message="confirm?.message ?? ''"
      :confirm-text="confirm?.confirmText"
      confirm-color="primary"
      :loading="saving"
      @confirm="save"
    />
  </div>
</template>
