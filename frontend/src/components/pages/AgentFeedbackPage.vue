<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { agentFeedbackApi } from "@/api/agentFeedback";
import type { AgentFeedbackItem, AgentFeedbackStatus } from "@/types/api";

const loading = ref(false);
const error = ref<string | null>(null);
const items = ref<AgentFeedbackItem[]>([]);
const updatingId = ref<string | null>(null);
const expandedId = ref<string | null>(null);

type FilterTab = "all" | AgentFeedbackStatus;
const activeTab = ref<FilterTab>("all");

const TABS: { value: FilterTab; title: string }[] = [
  { value: "all", title: "All" },
  { value: "pending", title: "Pending" },
  { value: "reviewed", title: "Reviewed" },
  { value: "actioned", title: "Actioned" },
  { value: "dismissed", title: "Dismissed" },
];

const CATEGORY_LABELS: Record<string, string> = {
  missing_tool: "Missing Tool",
  mcp_tuning: "MCP Tuning",
  workflow: "Workflow",
  documentation: "Documentation",
  other: "Other",
};

const CATEGORY_COLORS: Record<string, string> = {
  missing_tool: "error",
  mcp_tuning: "primary",
  workflow: "warning",
  documentation: "info",
  other: "secondary",
};

const STATUS_COLORS: Record<string, string> = {
  pending: "warning",
  reviewed: "info",
  actioned: "success",
  dismissed: "default",
};

const filteredItems = computed(() => {
  if (activeTab.value === "all") return items.value;
  return items.value.filter((item) => item.status === activeTab.value);
});

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const res = await agentFeedbackApi.list();
    items.value = res.items;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to load agent feedback";
  } finally {
    loading.value = false;
  }
}

async function updateStatus(id: string, status: AgentFeedbackStatus) {
  updatingId.value = id;
  try {
    const updated = await agentFeedbackApi.updateStatus(id, status);
    const idx = items.value.findIndex((i) => i.id === id);
    if (idx >= 0) {
      items.value[idx] = updated;
    }
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to update status";
  } finally {
    updatingId.value = null;
  }
}

function toggleExpanded(id: string) {
  expandedId.value = expandedId.value === id ? null : id;
}

function formatDate(dateStr: string): string {
  try {
    return new Date(dateStr).toLocaleString();
  } catch {
    return dateStr;
  }
}

onMounted(load);
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <div>
        <h3 class="text-h6">Agent Feedback</h3>
        <p class="text-body-2 text-secondary">
          Feedback recorded by agents via the <code>submit_agent_feedback</code> MCP tool.
          Review and action items to improve tooling and workflows.
        </p>
      </div>
      <v-spacer />
      <v-btn
        variant="tonal"
        size="small"
        prepend-icon="mdi-refresh"
        :loading="loading"
        @click="load"
      >
        Refresh
      </v-btn>
    </div>

    <v-alert v-if="error" type="error" class="mb-4" closable>
      {{ error }}
    </v-alert>

    <!-- Filter tabs -->
    <v-tabs v-model="activeTab" class="mb-4">
      <v-tab v-for="tab in TABS" :key="tab.value" :value="tab.value">
        {{ tab.title }}
        <v-chip
          v-if="tab.value === 'all'"
          size="x-small"
          class="ml-2"
          :color="items.length > 0 ? 'primary' : 'default'"
          variant="tonal"
        >{{ items.length }}</v-chip>
        <v-chip
          v-else
          size="x-small"
          class="ml-2"
          :color="STATUS_COLORS[tab.value] ?? 'default'"
          variant="tonal"
        >{{ items.filter((i) => i.status === tab.value).length }}</v-chip>
      </v-tab>
    </v-tabs>

    <!-- Loading state -->
    <div v-if="loading" class="d-flex justify-center pa-8">
      <v-progress-circular indeterminate color="primary" />
    </div>

    <!-- Empty state -->
    <div
      v-else-if="filteredItems.length === 0"
      class="text-center py-10 text-secondary"
    >
      <v-icon icon="mdi-message-text-outline" size="48" class="mb-3 text-disabled" />
      <div class="text-body-1">No agent feedback recorded yet</div>
      <div class="text-body-2 mt-1">
        Enable <strong>Agent Feedback</strong> in the Observability settings and connect
        an MCP client to start collecting feedback from agents.
      </div>
    </div>

    <!-- Feedback list -->
    <div v-else>
      <v-card
        v-for="item in filteredItems"
        :key="item.id"
        variant="outlined"
        class="mb-3"
      >
        <v-card-text class="pb-2">
          <div class="d-flex align-start ga-3 flex-wrap">
            <!-- Category chip -->
            <v-chip
              :color="CATEGORY_COLORS[item.category] ?? 'default'"
              size="small"
              variant="tonal"
              class="flex-shrink-0 mt-1"
            >
              {{ CATEGORY_LABELS[item.category] ?? item.category }}
            </v-chip>

            <!-- Summary and timestamp -->
            <div class="flex-grow-1">
              <div class="text-subtitle-2 font-weight-medium">{{ item.summary }}</div>
              <div class="text-caption text-secondary">
                {{ formatDate(item.created_at) }}
                <span v-if="item.reviewed_at" class="ml-2">
                  &middot; Reviewed {{ formatDate(item.reviewed_at) }}
                </span>
              </div>
            </div>

            <!-- Status chip -->
            <v-chip
              :color="STATUS_COLORS[item.status] ?? 'default'"
              size="small"
              variant="tonal"
              class="flex-shrink-0 mt-1"
            >
              {{ item.status }}
            </v-chip>
          </div>

          <!-- Expandable detail -->
          <div class="mt-2">
            <v-btn
              variant="text"
              size="x-small"
              :prepend-icon="expandedId === item.id ? 'mdi-chevron-up' : 'mdi-chevron-down'"
              @click="toggleExpanded(item.id)"
            >
              {{ expandedId === item.id ? "Hide detail" : "Show detail" }}
            </v-btn>

            <div v-if="expandedId === item.id" class="mt-2">
              <div class="text-body-2 text-medium-emphasis" style="white-space: pre-wrap">
                {{ item.detail }}
              </div>

              <template v-if="item.suggested_tool_name">
                <div class="text-caption text-secondary mt-2 font-weight-medium">
                  Suggested tool name
                </div>
                <v-chip size="small" variant="tonal" color="primary" class="mt-1">
                  {{ item.suggested_tool_name }}
                </v-chip>
              </template>

              <template v-if="item.suggested_prompt">
                <div class="text-caption text-secondary mt-2 font-weight-medium">
                  Suggested prompt
                </div>
                <v-card
                  variant="tonal"
                  color="primary"
                  class="mt-1 pa-3"
                  rounded="lg"
                >
                  <div class="text-body-2" style="white-space: pre-wrap">
                    {{ item.suggested_prompt }}
                  </div>
                </v-card>
              </template>
            </div>
          </div>
        </v-card-text>

        <!-- Actions -->
        <v-card-actions v-if="item.status === 'pending'" class="pt-0 px-4 pb-3">
          <v-btn
            size="small"
            variant="tonal"
            color="info"
            :loading="updatingId === item.id"
            @click="updateStatus(item.id, 'reviewed')"
          >
            Mark reviewed
          </v-btn>
          <v-btn
            size="small"
            variant="tonal"
            color="success"
            :loading="updatingId === item.id"
            @click="updateStatus(item.id, 'actioned')"
          >
            Mark actioned
          </v-btn>
          <v-btn
            size="small"
            variant="text"
            color="error"
            :loading="updatingId === item.id"
            @click="updateStatus(item.id, 'dismissed')"
          >
            Dismiss
          </v-btn>
        </v-card-actions>
        <v-card-actions v-else class="pt-0 px-4 pb-3">
          <v-btn
            size="small"
            variant="text"
            :loading="updatingId === item.id"
            @click="updateStatus(item.id, 'pending')"
          >
            Reset to pending
          </v-btn>
        </v-card-actions>
      </v-card>
    </div>
  </div>
</template>
