<script setup lang="ts">
// Settings → MCP Tuning → Kubernetes tool guardrails: the switches of the
// embedded mcp-k8s tools. The parent owns the value (it is part of the
// settings document it saves, as mcp_tuning.guardrails).
import { computed } from "vue";
import type { McpGuardrails, WorkloadPolicyMode } from "@/types/api";
import { GUARDRAIL_EXCEPTIONS, toggleException, validateApiGroup } from "@/utils/mcpGuardrails";

const model = defineModel<McpGuardrails>({ required: true });

const restrictive = computed(() => model.value.workload_policy === "restrictive");

function set<K extends keyof McpGuardrails>(key: K, value: McpGuardrails[K]) {
  model.value = { ...model.value, [key]: value };
}

function setMode(mode: WorkloadPolicyMode | null) {
  if (mode) set("workload_policy", mode);
}

function setException(field: string, on: boolean | null) {
  set("exceptions", toggleException(model.value.exceptions, field, on === true));
}

function setGroups(groups: string[] | null) {
  const cleaned = (groups ?? []).map((g) => g.trim().toLowerCase()).filter((g) => g !== "");
  set("allowed_custom_resource_groups", [...new Set(cleaned)]);
}

const groupRules = [
  (groups: string[] | null) => {
    for (const group of groups ?? []) {
      const error = validateApiGroup(group);
      if (error) return error;
    }
    return true;
  },
];
</script>

<template>
  <v-card variant="outlined" class="mb-4" data-testid="mcp-guardrails">
    <v-card-title class="text-subtitle-2 d-flex align-center ga-2">
      <v-icon icon="mdi-shield-check-outline" size="small" />
      Kubernetes tool guardrails
    </v-card-title>
    <v-card-text>
      <div class="text-caption text-medium-emphasis mb-4">
        What agents may do through the Kubernetes tools of deckwatch's MCP endpoint. A refused
        call returns an error that names the setting to change. Changes apply from the next MCP
        request; a client picks up a changed tool list when it reconnects.
      </div>

      <div class="text-subtitle-2 mb-2">Workload guardrails</div>
      <v-btn-toggle
        :model-value="model.workload_policy"
        mandatory
        divided
        density="compact"
        variant="outlined"
        color="primary"
        @update:model-value="setMode"
      >
        <v-btn value="restrictive">Restrictive (recommended)</v-btn>
        <v-btn value="permissive">Permissive</v-btn>
      </v-btn-toggle>

      <template v-if="restrictive">
        <div class="text-caption text-medium-emphasis mt-2 mb-4">
          The typed create and update tools and the custom resource tools refuse privileged
          containers, host namespaces, hostPath volumes and host ports, extra capabilities,
          NodePort, LoadBalancer and ExternalName Services, ingress snippet annotations, RBAC
          escalation, and custom resources outside the API groups below.
        </div>

        <div class="text-subtitle-2">Exceptions</div>
        <div class="text-caption text-medium-emphasis mb-1">
          Rules the restrictive policy lets through anyway.
        </div>
        <v-row dense class="mb-2">
          <v-col v-for="rule in GUARDRAIL_EXCEPTIONS" :key="rule.field" cols="12" md="6">
            <v-checkbox
              :model-value="model.exceptions.includes(rule.field)"
              color="primary"
              density="compact"
              hide-details
              @update:model-value="(on: boolean | null) => setException(rule.field, on)"
            >
              <template #label>
                <span>{{ rule.label }}</span>
                <code class="ml-2 text-caption text-medium-emphasis text-no-wrap">{{ rule.field }}</code>
              </template>
            </v-checkbox>
          </v-col>
        </v-row>

        <v-combobox
          :model-value="model.allowed_custom_resource_groups"
          label="Allowed custom resource API groups"
          placeholder="e.g. cert-manager.io"
          :rules="groupRules"
          multiple
          chips
          closable-chips
          variant="outlined"
          density="comfortable"
          hint="create_custom_resource and update_custom_resource may write custom resources of these API groups (* for any). With none, they refuse every write: an operator can turn a custom resource into pods or RBAC objects."
          persistent-hint
          @update:model-value="setGroups"
        />
      </template>
      <v-alert v-else type="warning" variant="tonal" density="compact" class="mt-3">
        Every workload guardrail is off. Agents can create privileged pods, mount host paths,
        open host ports, expose LoadBalancer Services and bind cluster-admin through the typed
        tools, and write any custom resource. Only deckwatch's Kubernetes RBAC and your admission
        policies still limit them.
      </v-alert>

      <v-divider class="my-4" />

      <v-switch
        :model-value="model.apply_manifest_enabled"
        color="primary"
        label="apply_manifest (raw manifests)"
        inset
        density="compact"
        hide-details
        @update:model-value="(on: boolean | null) => set('apply_manifest_enabled', on === true)"
      />
      <div class="text-caption text-medium-emphasis mb-4">
        Agents can apply any manifest with a server-side apply. It bypasses the workload
        guardrails above, so it can create anything deckwatch's service account may, such as
        privileged pods, RBAC bindings, webhooks and CRDs; only the namespace allowlist still
        applies. MCP clients such as Claude Code ask the user to approve each call.
      </div>

      <v-switch
        :model-value="model.secret_decode_enabled"
        color="primary"
        label="Return Secret values"
        inset
        density="compact"
        hide-details
        @update:model-value="(on: boolean | null) => set('secret_decode_enabled', on === true)"
      />
      <div class="text-caption text-medium-emphasis mb-4">
        <code>get_secret</code> can return decoded Secret values, and
        <code>get_resource_json</code> a Secret's data. The values then end up in the agent's
        context and the MCP client's transcript. When off, agents see a Secret's keys, never its
        values.
      </div>

      <v-switch
        :model-value="model.allow_privileged_csr"
        color="primary"
        label="Approve privileged certificate requests"
        inset
        density="compact"
        hide-details
        @update:model-value="(on: boolean | null) => set('allow_privileged_csr', on === true)"
      />
      <div class="text-caption text-medium-emphasis mb-4">
        <code>approve_csr</code> can approve CertificateSigningRequests for
        <code>system:masters</code> and other <code>system:</code> users and groups. Such a
        certificate can act as cluster-admin or as a node. Applies in either policy mode.
      </div>

      <v-switch
        :model-value="model.secure_defaults"
        color="primary"
        label="Secure defaults for new workloads"
        inset
        density="compact"
        hide-details
        @update:model-value="(on: boolean | null) => set('secure_defaults', on === true)"
      />
      <div class="text-caption text-medium-emphasis">
        The typed create tools set runAsNonRoot, the RuntimeDefault seccomp profile, no privilege
        escalation, all capabilities dropped and no service account token, unless the agent sets
        them. An image that must run as root needs
        <code>security_context.run_as_non_root: false</code>. Applies in either policy mode.
      </div>
    </v-card-text>
  </v-card>
</template>
