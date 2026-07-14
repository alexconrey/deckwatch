<script setup lang="ts">
import { ref, computed, onUnmounted, watch } from "vue";
import { previewApi } from "@/api/preview";
import { gitopsApi } from "@/api/gitops";
import { ApiError } from "@/api/client";
import type {
  CreatePreviewRequest,
  PreviewSummary,
  BranchListResponse,
} from "@/types/api";
import { formatAge } from "@/utils/format";

const props = defineProps<{
  namespace: string;
  deploymentName: string;
  // Passed through from GitOpsCard: needed so the branch dropdown can
  // resolve the same token secret the GitOps config uses. When absent,
  // the branch dropdown falls back to a free-text input.
  repoUrl?: string | null;
  tokenSecret?: string | null;
  // Present-tense enabled state from GitOpsCard. When false we render
  // nothing — previews without GitOps have no way to build.
  gitopsEnabled: boolean;
}>();

const previews = ref<PreviewSummary[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

const showCreateDialog = ref(false);
const createBranch = ref("");
const createPr = ref<number | null>(null);
const createHostSuffix = ref("");
const createTtlHours = ref<number>(24);
const creating = ref(false);
const createError = ref<string | null>(null);

const branchOptions = ref<string[]>([]);
const branchesLoading = ref(false);
const branchesError = ref<string | null>(null);

const deleteTarget = ref<string | null>(null);
const deleting = ref(false);

async function refresh() {
  loading.value = true;
  error.value = null;
  try {
    const res = await previewApi.listForSource(
      props.namespace,
      props.deploymentName,
    );
    previews.value = res.previews;
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e.body?.message ?? e.message
        : e instanceof Error
          ? e.message
          : "Failed to load previews";
  } finally {
    loading.value = false;
  }
}

// Refresh whenever gitops is enabled (or toggled off) — turning it off
// hides the card entirely, but toggling back on should re-fetch instead
// of showing whatever list was cached from an earlier session.
watch(
  () => props.gitopsEnabled,
  (enabled) => {
    if (enabled) void refresh();
    else previews.value = [];
  },
  { immediate: true },
);

// Poll live so the TTL countdown ticks and expiring previews vanish from
// the list without a manual refresh. 15s is slower than the general 5s
// deployment poll to avoid pounding the API on a large namespace.
let pollHandle: ReturnType<typeof setInterval> | null = null;
watch(
  () => props.gitopsEnabled,
  (enabled) => {
    if (pollHandle) {
      clearInterval(pollHandle);
      pollHandle = null;
    }
    if (enabled) {
      pollHandle = setInterval(refresh, 15000);
    }
  },
  { immediate: true },
);

onUnmounted(() => {
  if (pollHandle) clearInterval(pollHandle);
});

async function loadBranchOptions() {
  branchOptions.value = [];
  branchesError.value = null;
  if (!props.repoUrl || !props.tokenSecret) {
    // Missing token secret is normal for public repos. The dropdown
    // degrades to plain text input; not surfacing an error prevents the
    // dialog from looking broken when the user is on the happy path.
    return;
  }
  branchesLoading.value = true;
  try {
    const res: BranchListResponse = await gitopsApi.listBranches({
      repoUrl: props.repoUrl,
      tokenSecret: props.tokenSecret,
      namespace: props.namespace,
    });
    branchOptions.value = res.branches.map((b) => b.name);
  } catch (e) {
    branchesError.value =
      e instanceof ApiError
        ? e.body?.message ?? e.message
        : e instanceof Error
          ? e.message
          : "Failed to list branches";
  } finally {
    branchesLoading.value = false;
  }
}

function openCreateDialog() {
  createBranch.value = "";
  createPr.value = null;
  createHostSuffix.value = "";
  createTtlHours.value = 24;
  createError.value = null;
  showCreateDialog.value = true;
  void loadBranchOptions();
}

async function submitCreate() {
  const branch = createBranch.value.trim();
  if (!branch) {
    createError.value = "Branch is required";
    return;
  }
  const body: CreatePreviewRequest = {
    branch,
    pr_number: createPr.value ?? undefined,
    host_suffix: createHostSuffix.value.trim() || undefined,
    ttl_hours: createTtlHours.value,
  };
  creating.value = true;
  createError.value = null;
  try {
    await previewApi.create(props.namespace, props.deploymentName, body);
    showCreateDialog.value = false;
    await refresh();
  } catch (e) {
    createError.value =
      e instanceof ApiError
        ? e.body?.message ?? e.message
        : e instanceof Error
          ? e.message
          : "Failed to create preview";
  } finally {
    creating.value = false;
  }
}

async function confirmDelete() {
  if (!deleteTarget.value) return;
  deleting.value = true;
  try {
    await previewApi.delete(props.namespace, deleteTarget.value);
    deleteTarget.value = null;
    await refresh();
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e.body?.message ?? e.message
        : e instanceof Error
          ? e.message
          : "Failed to delete preview";
  } finally {
    deleting.value = false;
  }
}

// Countdown to expiry. Recomputed by tying it to a reactive `now` that
// bumps once a minute — good enough resolution for a 24h default TTL,
// and cheap enough not to warrant a per-second re-render.
const now = ref(Date.now());
let nowInterval: ReturnType<typeof setInterval> | null = null;
watch(
  () => props.gitopsEnabled,
  (enabled) => {
    if (nowInterval) {
      clearInterval(nowInterval);
      nowInterval = null;
    }
    if (enabled) {
      nowInterval = setInterval(() => (now.value = Date.now()), 60_000);
    }
  },
  { immediate: true },
);
onUnmounted(() => {
  if (nowInterval) clearInterval(nowInterval);
});

function timeUntilExpiry(iso: string): string {
  if (!iso) return "";
  const expiresMs = Date.parse(iso);
  if (Number.isNaN(expiresMs)) return "unknown";
  const delta = expiresMs - now.value;
  if (delta <= 0) return "expired";
  const minutes = Math.floor(delta / 60_000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return `${hours}h ${minutes % 60}m`;
  return `${Math.floor(hours / 24)}d`;
}

function expiryColor(iso: string): string {
  if (!iso) return "secondary";
  const delta = Date.parse(iso) - now.value;
  if (delta <= 0) return "error";
  if (delta < 60 * 60_000) return "warning";
  return "success";
}

const previewCount = computed(() => previews.value.length);
</script>

<template>
  <v-card v-if="gitopsEnabled" class="mb-4">
    <v-card-title class="d-flex align-center">
      <v-icon icon="mdi-eye-outline" class="mr-2" size="small" />
      <span class="text-subtitle-1">Preview Environments</span>
      <v-chip v-if="previewCount > 0" size="x-small" class="ml-2" variant="tonal">
        {{ previewCount }}
      </v-chip>
      <v-spacer />
      <v-btn
        size="small"
        variant="text"
        prepend-icon="mdi-plus"
        @click="openCreateDialog"
      >
        New Preview
      </v-btn>
      <v-btn
        icon="mdi-refresh"
        size="small"
        variant="text"
        :loading="loading"
        @click="refresh"
      />
    </v-card-title>

    <v-alert v-if="error" type="error" density="compact" class="mx-4 mb-2" closable>
      {{ error }}
    </v-alert>

    <v-table v-if="previews.length > 0" density="compact">
      <thead>
        <tr>
          <th>Preview</th>
          <th>Branch / PR</th>
          <th>URL</th>
          <th>Ready</th>
          <th>Age</th>
          <th>Expires in</th>
          <th style="width: 60px"></th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="p in previews" :key="p.name">
          <td class="text-caption font-weight-medium">{{ p.name }}</td>
          <td>
            <div class="d-flex align-center ga-1">
              <v-chip size="x-small" variant="outlined">
                <v-icon icon="mdi-source-branch" start size="x-small" />
                {{ p.branch }}
              </v-chip>
              <v-chip
                v-if="p.pr_number"
                size="x-small"
                color="info"
                variant="tonal"
              >
                #{{ p.pr_number }}
              </v-chip>
            </div>
          </td>
          <td class="text-caption">
            <a
              v-if="p.host"
              :href="`https://${p.host}`"
              target="_blank"
              rel="noopener"
            >
              {{ p.host }}
            </a>
            <span v-else class="text-secondary">port-forward only</span>
          </td>
          <td class="text-caption">
            {{ p.replicas_ready }} / {{ p.replicas_desired }}
          </td>
          <td class="text-caption text-secondary">
            {{ formatAge(p.created_at, { suffix: " ago" }) }}
          </td>
          <td>
            <v-chip
              size="x-small"
              :color="expiryColor(p.expires_at)"
              variant="tonal"
            >
              {{ timeUntilExpiry(p.expires_at) }}
            </v-chip>
          </td>
          <td>
            <v-btn
              icon="mdi-delete"
              size="x-small"
              variant="text"
              color="error"
              @click="deleteTarget = p.name"
            />
          </td>
        </tr>
      </tbody>
    </v-table>

    <div
      v-else-if="!loading"
      class="text-center py-4 text-secondary text-body-2"
    >
      No preview environments. Create one to spin up a temporary copy of this
      deployment tracking a feature branch.
    </div>

    <!-- Create dialog -->
    <v-dialog v-model="showCreateDialog" max-width="560">
      <v-card>
        <v-card-title>Create Preview Environment</v-card-title>
        <v-card-text>
          <div class="text-body-2 mb-3 text-secondary">
            Clones
            <strong>{{ deploymentName }}</strong>
            with the branch pinned to the selected branch. Kaniko will build
            the image; the preview auto-deletes when the TTL expires.
          </div>

          <v-alert
            v-if="createError"
            type="error"
            density="compact"
            class="mb-3"
            closable
            @click:close="createError = null"
          >
            {{ createError }}
          </v-alert>

          <v-alert
            v-if="branchesError"
            type="warning"
            density="compact"
            class="mb-3"
          >
            Could not list branches: {{ branchesError }}. You can still type a
            branch name manually.
          </v-alert>

          <v-combobox
            v-model="createBranch"
            :items="branchOptions"
            :loading="branchesLoading"
            label="Branch"
            density="compact"
            class="mb-2"
            hint="Existing branch on the source repo — the preview will track it and rebuild on push."
            persistent-hint
          />

          <v-text-field
            v-model.number="createPr"
            label="PR Number (optional)"
            type="number"
            density="compact"
            class="mb-2"
          />

          <v-text-field
            v-model="createHostSuffix"
            label="Host Suffix (optional)"
            placeholder="preview.example.com"
            density="compact"
            hint="Preview will be reachable at {branch}.preview.{suffix}. Leave blank to use port-forward only."
            persistent-hint
            class="mb-2"
          />

          <v-slider
            v-model="createTtlHours"
            :min="1"
            :max="168"
            :step="1"
            label="TTL (hours)"
            density="compact"
            thumb-label="always"
            hide-details
            class="mt-3"
          />
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="showCreateDialog = false">Cancel</v-btn>
          <v-btn
            color="primary"
            variant="flat"
            :loading="creating"
            :disabled="!createBranch.trim()"
            @click="submitCreate"
          >
            Create
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <!-- Delete confirmation -->
    <v-dialog
      :model-value="!!deleteTarget"
      max-width="440"
      @update:model-value="(v) => !v && (deleteTarget = null)"
    >
      <v-card>
        <v-card-title>Delete Preview</v-card-title>
        <v-card-text>
          Deletes preview
          <strong>{{ deleteTarget }}</strong>
          and its Ingress. The source deployment is not affected.
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="deleteTarget = null">Cancel</v-btn>
          <v-btn
            color="error"
            variant="flat"
            :loading="deleting"
            @click="confirmDelete"
          >
            Delete
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </v-card>
</template>
