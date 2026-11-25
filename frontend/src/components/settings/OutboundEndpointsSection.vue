<script setup lang="ts">
// Settings → Advanced → Endpoints: overrides for the AI provider and GitHub
// endpoints, the deployment's outbound TLS policy, and where each stored
// credential may be sent. The parent owns the endpoint values (they are part
// of the settings document it saves); this component loads
// GET /api/settings/outbound for the defaults and binding state.
import { computed, onMounted, ref } from "vue";
import { settingsApi } from "@/api/settings";
import type { CredentialStatus, EndpointSettings, OutboundStatus } from "@/types/api";
import { ENDPOINT_FIELDS, type EndpointField, validateEndpoint } from "@/utils/endpoints";

const model = defineModel<EndpointSettings>({ required: true });

const status = ref<OutboundStatus | null>(null);
const statusError = ref<string | null>(null);

async function refresh() {
  try {
    status.value = await settingsApi.outbound();
    statusError.value = null;
  } catch (e) {
    statusError.value = e instanceof Error ? e.message : "Failed to load endpoint status";
  }
}

onMounted(refresh);
defineExpose({ refresh });

const allowInsecure = computed(() => status.value?.allow_insecure_endpoints ?? false);
const aiFields = ENDPOINT_FIELDS.filter((f) => f.group === "ai");
const githubFields = ENDPOINT_FIELDS.filter((f) => f.group === "github");

function serverStatus(field: EndpointField) {
  return status.value?.endpoints.find((e) => e.id === field.id);
}

function placeholder(field: EndpointField): string {
  return serverStatus(field)?.default ?? field.defaultUrl;
}

function rules(field: EndpointField) {
  return [(v: string | null) => validateEndpoint(field, v ?? "", allowInsecure.value) ?? true];
}

// Clearing a v-text-field yields null; the API expects "" for "default".
function update(field: EndpointField, value: string | null) {
  model.value = { ...model.value, [field.setting]: value ?? "" };
}

function boundHosts(c: CredentialStatus): string {
  return [...new Set(c.bindings.map((b) => b.bound_host))].join(", ");
}

function movedHosts(c: CredentialStatus): string {
  return [
    ...new Set(c.bindings.filter((b) => !b.matches).map((b) => b.configured_host || "an invalid endpoint")),
  ].join(", ");
}
</script>

<template>
  <div data-testid="outbound-endpoints">
    <div class="text-subtitle-1 font-weight-bold mb-1">Endpoints</div>
    <p class="text-body-2 text-medium-emphasis mb-3">
      Where deckwatch sends AI provider and GitHub requests: an LLM gateway, a
      private or VPC endpoint, GitHub Enterprise. Leave a field empty to use the
      default shown. A stored credential is only sent to the host it was saved
      for, so after changing an endpoint, re-enter the credential.
    </p>

    <v-alert
      v-if="statusError"
      type="warning"
      variant="tonal"
      density="compact"
      class="mb-3"
      data-testid="outbound-status-error"
    >
      {{ statusError }}
    </v-alert>

    <div class="d-flex flex-wrap ga-2 mb-4">
      <v-chip
        size="small"
        variant="tonal"
        :color="allowInsecure ? 'warning' : 'success'"
        :prepend-icon="allowInsecure ? 'mdi-lock-open-alert' : 'mdi-lock'"
        data-testid="outbound-policy"
      >
        {{
          allowInsecure
            ? "http:// allowed for any host (DECKWATCH_ALLOW_INSECURE_ENDPOINTS)"
            : "https:// only (http:// for localhost)"
        }}
      </v-chip>
      <v-chip
        size="small"
        variant="tonal"
        prepend-icon="mdi-certificate"
        data-testid="outbound-extra-ca"
      >
        <template v-if="status?.extra_ca">
          System roots + {{ status.extra_ca.certificates }} extra CA
          {{ status.extra_ca.certificates === 1 ? "certificate" : "certificates" }}
          ({{ status.extra_ca.file }})
        </template>
        <template v-else>System trust store</template>
      </v-chip>
    </div>

    <div class="text-overline">AI providers</div>
    <v-row dense>
      <v-col v-for="field in aiFields" :key="field.setting" cols="12" md="6">
        <v-text-field
          :model-value="model[field.setting]"
          :label="field.label"
          :placeholder="placeholder(field)"
          persistent-placeholder
          :hint="field.hint"
          persistent-hint
          :rules="rules(field)"
          :error-messages="serverStatus(field)?.error ?? []"
          clearable
          variant="outlined"
          density="compact"
          :data-testid="`endpoint-${field.setting}`"
          @update:model-value="update(field, $event)"
        />
      </v-col>
    </v-row>

    <div class="text-overline mt-2">GitHub</div>
    <v-row dense>
      <v-col v-for="field in githubFields" :key="field.setting" cols="12" md="6">
        <v-text-field
          :model-value="model[field.setting]"
          :label="field.label"
          :placeholder="placeholder(field)"
          persistent-placeholder
          :hint="field.hint"
          persistent-hint
          :rules="rules(field)"
          :error-messages="serverStatus(field)?.error ?? []"
          clearable
          variant="outlined"
          density="compact"
          :data-testid="`endpoint-${field.setting}`"
          @update:model-value="update(field, $event)"
        />
      </v-col>
    </v-row>

    <div class="text-overline mt-2">Stored credentials</div>
    <v-list density="compact" class="pa-0" data-testid="credential-bindings">
      <v-list-item
        v-for="c in status?.credentials ?? []"
        :key="c.credential"
        :data-testid="`binding-${c.credential}`"
        class="px-0"
      >
        <template #prepend>
          <v-icon
            :icon="c.usable ? 'mdi-lock-check' : 'mdi-lock-alert'"
            :color="c.usable ? 'success' : 'warning'"
          />
        </template>
        <v-list-item-title>{{ c.label }} saved for {{ boundHosts(c) }}</v-list-item-title>
        <v-list-item-subtitle v-if="!c.usable" class="text-warning">
          The endpoint now points to {{ movedHosts(c) }}. The {{ c.label }} is not
          sent there; re-enter it to use it with the new host.
        </v-list-item-subtitle>
      </v-list-item>
      <v-list-item
        v-if="status && status.credentials.length === 0"
        class="px-0 text-medium-emphasis"
        data-testid="credential-bindings-empty"
      >
        No credentials are stored in deckwatch.
      </v-list-item>
    </v-list>
    <p class="text-caption text-medium-emphasis mt-1">
      Endpoint changes take effect when you save and are recorded in the audit log.
    </p>
  </div>
</template>
