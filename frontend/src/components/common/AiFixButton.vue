<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { DiagAgent } from "@/types/api";
import { useAiSettings } from "@/composables/useAiSettings";

// Reusable "Fix with AI" trigger.
//
// Presents the same agent-picker UX as the DiagnoseButton, but is intentionally
// independent so the button can live on any resource surface (Applications,
// Deployments, etc.) without dragging in DiagnoseButton's pod-log plumbing.
//
// The actual "fix" work is expected to hit a POST
// /api/namespaces/{ns}/applications/{name}/ai-fix endpoint. The parent
// component is responsible for making that call and handling the response —
// this component just resolves an agent and emits `fix`.

const props = defineProps<{
  // Text displayed on the button. Defaults to "Fix with AI".
  label?: string;
  // When true the button is rendered but disabled with a tooltip explaining
  // why (e.g. GitOps not enabled). Use this instead of v-if so the affordance
  // stays discoverable.
  disabled?: boolean;
  disabledReason?: string;
  // Optional: skip the chooser and go straight to `fix` when this agent is
  // already known to be selected. Callers can persist a preference elsewhere.
  preselectedAgent?: DiagAgent | null;
}>();

const emit = defineEmits<{
  (e: "fix", agent: DiagAgent): void;
}>();

const AGENT_KEY = "deckwatch-ai-agent";

const showChooser = ref(false);
const savedAgent = ref<DiagAgent | null>(
  props.preselectedAgent ??
    (sessionStorage.getItem(AGENT_KEY) as DiagAgent | null) ??
    null,
);

const { claudeEnabled, codexEnabled } = useAiSettings();

const anyProviderEnabled = computed(
  () => claudeEnabled.value || codexEnabled.value,
);

// Same self-heal as DiagnoseButton: forget an agent that has since been
// disabled so the chooser reopens on the next click.
watch(
  [savedAgent, claudeEnabled, codexEnabled],
  () => {
    if (savedAgent.value === "claude" && !claudeEnabled.value) {
      savedAgent.value = null;
      sessionStorage.removeItem(AGENT_KEY);
    }
    if (savedAgent.value === "codex" && !codexEnabled.value) {
      savedAgent.value = null;
      sessionStorage.removeItem(AGENT_KEY);
    }
  },
  { immediate: true },
);

const effectiveDisabled = computed(
  () => props.disabled || !anyProviderEnabled.value,
);

const effectiveReason = computed(() => {
  if (!anyProviderEnabled.value) {
    return "Enable an AI provider under Settings → AI Integrations.";
  }
  return props.disabledReason ?? "";
});

function selectAgent(agent: DiagAgent) {
  if (agent === "codex" && !codexEnabled.value) return;
  if (agent === "claude" && !claudeEnabled.value) return;
  savedAgent.value = agent;
  sessionStorage.setItem(AGENT_KEY, agent);
  showChooser.value = false;
  emit("fix", agent);
}

function onClick() {
  if (effectiveDisabled.value) return;
  if (savedAgent.value) {
    emit("fix", savedAgent.value);
  } else {
    showChooser.value = true;
  }
}

</script>

<template>
  <div class="ai-fix-container">
    <v-tooltip
      :text="effectiveReason"
      location="bottom"
      :disabled="!effectiveDisabled || !effectiveReason"
    >
      <template #activator="{ props: tipProps }">
        <div v-bind="tipProps" class="d-inline-block">
          <v-btn
            color="secondary"
            variant="tonal"
            prepend-icon="mdi-auto-fix"
            size="small"
            :disabled="effectiveDisabled"
            @click="onClick"
          >
            {{ label ?? "Fix with AI" }}
          </v-btn>
        </div>
      </template>
    </v-tooltip>

    <v-dialog v-model="showChooser" max-width="640">
      <v-card>
        <v-card-title class="d-flex align-center">
          <v-icon icon="mdi-auto-fix" class="mr-2" />
          Choose an AI agent
        </v-card-title>
        <v-card-subtitle>
          The selected agent will be asked to propose fixes for this
          application's source repository.
        </v-card-subtitle>
        <v-card-text>
          <v-row>
            <v-col cols="12" sm="6">
              <v-card
                variant="outlined"
                class="pa-4 agent-card"
                :class="{ 'agent-card-disabled': !claudeEnabled }"
                @click="claudeEnabled && selectAgent('claude')"
              >
                <div class="d-flex align-center mb-2">
                  <v-icon icon="mdi-alpha-c-circle" color="deep-purple" />
                  <span class="text-h6 ml-2">Claude</span>
                  <v-chip
                    v-if="!claudeEnabled"
                    size="x-small"
                    color="warning"
                    variant="tonal"
                    class="ml-2"
                  >
                    Disabled
                  </v-chip>
                </div>
                <div class="text-body-2 text-secondary">
                  Anthropic Claude Code CLI. Reads the whole repo, drafts
                  targeted edits, and returns a diff you can review before
                  applying.
                </div>
              </v-card>
            </v-col>
            <v-col cols="12" sm="6">
              <v-card
                variant="outlined"
                class="pa-4 agent-card agent-card-disabled"
              >
                <div class="d-flex align-center mb-2">
                  <v-icon icon="mdi-alpha-o-circle" color="grey" />
                  <span class="text-h6 ml-2">Codex</span>
                  <v-chip
                    size="x-small"
                    color="info"
                    variant="tonal"
                    class="ml-2"
                  >
                    Coming Soon
                  </v-chip>
                </div>
                <div class="text-body-2 text-secondary">
                  OpenAI Codex CLI. Strong on symbol-level code and concise
                  remediation suggestions.
                </div>
              </v-card>
            </v-col>
          </v-row>
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="showChooser = false">Cancel</v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>
.ai-fix-container {
  display: inline-block;
}

.agent-card {
  cursor: pointer;
  transition: border-color 120ms ease;
}
.agent-card:hover {
  border-color: rgb(var(--v-theme-primary));
}
.agent-card-disabled {
  cursor: not-allowed;
  opacity: 0.55;
}
.agent-card-disabled:hover {
  border-color: rgba(var(--v-border-color), var(--v-border-opacity));
}
</style>
