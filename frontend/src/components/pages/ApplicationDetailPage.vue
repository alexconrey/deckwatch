<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { applicationsApi } from "@/api/applications";
import { usePolling } from "@/composables/usePolling";
import { usePodMetrics } from "@/composables/useResourceMetrics";
import type {
  ApplicationDetail,
  ApplicationGitConfig,
  DiagAgent,
  UpdateApplicationRequest,
} from "@/types/api";
import ApplicationHealthChip from "@/components/common/ApplicationHealthChip.vue";
import StatusChip from "@/components/common/StatusChip.vue";
import AddMemberDialog from "@/components/common/AddMemberDialog.vue";
import MetricSparkline from "@/components/common/MetricSparkline.vue";
import AiFixButton from "@/components/common/AiFixButton.vue";
import { formatAge } from "@/utils/format";

const props = defineProps<{
  namespace: string;
  name: string;
}>();

const router = useRouter();
const detail = ref<ApplicationDetail | null>(null);
const loading = ref(true);
const error = ref<string | null>(null);
const actionLoading = ref(false);

const showDeleteDialog = ref(false);
const cascadeDelete = ref(false);
const typedDeleteConfirm = ref("");
const showEditDialog = ref(false);
const showAddMemberDialog = ref(false);

// Fix-with-AI transient UI state. The backend endpoint that would actually
// dispatch the fix job does not exist yet, so for now we surface a notice
// explaining what will happen once it lands.
const fixNotice = ref<{ agent: DiagAgent } | null>(null);

// Reset the type-to-confirm text each time the delete dialog opens so a
// stale value from a prior aborted attempt can't re-arm the Delete button.
watch(showDeleteDialog, (open) => {
  if (open) {
    typedDeleteConfirm.value = "";
    cascadeDelete.value = false;
  }
});

const deleteConfirmMatches = computed(
  () => typedDeleteConfirm.value === props.name,
);

const gitopsEnabled = computed<boolean>(() => detail.value?.git != null);

const editDescription = ref("");
const editHasGit = ref(false);
const editGit = ref<ApplicationGitConfig>({
  repo_url: "",
  branch: "main",
  token_secret: "",
});

const fetchDetail = async () => {
  try {
    detail.value = await applicationsApi.get(props.namespace, props.name);
    loading.value = false;
  } catch (e) {
    error.value =
      e instanceof Error ? e.message : "Failed to fetch application";
    loading.value = false;
  }
};

usePolling(fetchDetail, 5000);

// Applications are logical groupings -- there is no single label that
// selects all their pods. Fetch the whole namespace and filter to pods
// whose name has a deployment prefix. Cheap for a dashboard view; if a
// namespace has thousands of pods, revisit with a proper selector strategy.
const namespaceRef = computed(() => props.namespace);
const { series: nsPodSeries, latest: nsPodLatest, unavailableReason: metricsUnavailable } =
  usePodMetrics(namespaceRef);

const memberDeploymentNames = computed<string[]>(
  () => detail.value?.deployments.map((d) => d.name) ?? [],
);

const belongsToApp = (podName: string): boolean => {
  for (const dep of memberDeploymentNames.value) {
    // ReplicaSet-managed pods are `<deployment>-<rs-hash>-<pod-hash>`.
    if (podName === dep || podName.startsWith(`${dep}-`)) return true;
  }
  return false;
};

const appPodCpu = computed<number>(() => {
  let total = 0;
  for (const p of nsPodLatest.value.values()) {
    if (belongsToApp(p.name)) total += p.total_cpu_millicores;
  }
  return total;
});
const appPodMem = computed<number>(() => {
  let total = 0;
  for (const p of nsPodLatest.value.values()) {
    if (belongsToApp(p.name)) total += p.total_memory_bytes;
  }
  return total;
});

const appCpuSeries = computed<number[]>(() => {
  const buffers = Array.from(nsPodSeries.value.entries())
    .filter(([name]) => belongsToApp(name))
    .map(([, s]) => s.samples);
  if (buffers.length === 0) return [];
  const maxLen = Math.max(...buffers.map((b) => b.length));
  const out: number[] = [];
  for (let i = 0; i < maxLen; i++) {
    let sum = 0;
    for (const buf of buffers) {
      const s = buf[buf.length - maxLen + i];
      if (s) sum += s.cpuMillicores;
    }
    out.push(sum);
  }
  return out;
});
const appMemSeries = computed<number[]>(() => {
  const buffers = Array.from(nsPodSeries.value.entries())
    .filter(([name]) => belongsToApp(name))
    .map(([, s]) => s.samples);
  if (buffers.length === 0) return [];
  const maxLen = Math.max(...buffers.map((b) => b.length));
  const out: number[] = [];
  for (let i = 0; i < maxLen; i++) {
    let sum = 0;
    for (const buf of buffers) {
      const s = buf[buf.length - maxLen + i];
      if (s) sum += s.memBytes;
    }
    out.push(sum);
  }
  return out;
});

const formatCpuTotal = (mcpu: number): string => {
  if (mcpu >= 1000) return `${(mcpu / 1000).toFixed(2)} cores`;
  return `${mcpu}m`;
};
const formatMemTotal = (bytes: number): string => {
  const mib = bytes / (1024 * 1024);
  if (mib >= 1024) return `${(mib / 1024).toFixed(1)} GiB`;
  return `${Math.round(mib)} MiB`;
};

const deploymentHeaders = [
  { title: "Name", key: "name" },
  { title: "Image", key: "image" },
  { title: "Status", key: "status", width: "140px" },
  { title: "Replicas", key: "replicas", width: "120px" },
  { title: "", key: "actions", width: "60px", sortable: false },
];

const cronjobHeaders = [
  { title: "Name", key: "name" },
  { title: "Schedule", key: "schedule", width: "160px" },
  { title: "Suspend", key: "suspend", width: "100px" },
  { title: "Active", key: "active_count", width: "100px" },
  { title: "", key: "actions", width: "60px", sortable: false },
];

const goToDeployment = (name: string) => {
  router.push({
    name: "DeploymentDetail",
    params: { namespace: props.namespace, name },
  });
};

const openEdit = () => {
  if (!detail.value) return;
  editDescription.value = detail.value.description;
  editHasGit.value = detail.value.git !== null;
  editGit.value = detail.value.git
    ? {
        repo_url: detail.value.git.repo_url,
        branch: detail.value.git.branch ?? "main",
        token_secret: detail.value.git.token_secret ?? "",
      }
    : { repo_url: "", branch: "main", token_secret: "" };
  showEditDialog.value = true;
};

const handleSaveEdit = async () => {
  actionLoading.value = true;
  try {
    const body: UpdateApplicationRequest = {
      description: editDescription.value,
      git: editHasGit.value
        ? {
            repo_url: editGit.value.repo_url,
            branch: editGit.value.branch || undefined,
            token_secret: editGit.value.token_secret || undefined,
          }
        : undefined,
    };
    detail.value = await applicationsApi.update(
      props.namespace,
      props.name,
      body,
    );
    showEditDialog.value = false;
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to update";
  } finally {
    actionLoading.value = false;
  }
};

const handleDelete = async () => {
  actionLoading.value = true;
  try {
    await applicationsApi.delete(props.namespace, props.name, cascadeDelete.value);
    showDeleteDialog.value = false;
    router.push({ name: "Applications" });
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to delete";
  } finally {
    actionLoading.value = false;
  }
};

const handleRemoveMember = async (kind: string, resourceName: string) => {
  actionLoading.value = true;
  try {
    detail.value = await applicationsApi.removeMember(
      props.namespace,
      props.name,
      kind,
      resourceName,
    );
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to remove resource";
  } finally {
    actionLoading.value = false;
  }
};

const onMemberAdded = (updated: ApplicationDetail) => {
  detail.value = updated;
};

const onFixWithAi = (agent: DiagAgent) => {
  // TODO(backend): POST /api/v1/namespaces/{ns}/applications/{name}/ai-fix
  //   body: { agent }
  //   response: { job_name, status } (mirrors diagnostics)
  // Until the backend lands we just surface an inline notice so the button
  // still gives the operator feedback that their intent was captured.
  fixNotice.value = { agent };
};
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <v-btn
        icon="mdi-arrow-left"
        variant="text"
        @click="router.push({ name: 'Applications' })"
      />
      <div class="ml-2">
        <h2 class="text-h5">{{ name }}</h2>
        <span class="text-caption text-secondary">{{ namespace }}</span>
      </div>
      <ApplicationHealthChip
        v-if="detail"
        :health="detail.health"
        class="ml-3"
      />
      <v-spacer />
      <div v-if="detail" class="d-flex ga-2">
        <AiFixButton
          v-if="gitopsEnabled"
          @fix="onFixWithAi"
        />
        <v-btn
          variant="outlined"
          size="small"
          prepend-icon="mdi-pencil"
          @click="openEdit"
        >
          Edit
        </v-btn>
        <v-btn
          variant="outlined"
          size="small"
          prepend-icon="mdi-plus"
          @click="showAddMemberDialog = true"
        >
          Add Resource
        </v-btn>
        <v-btn
          variant="outlined"
          size="small"
          color="error"
          prepend-icon="mdi-delete"
          @click="showDeleteDialog = true"
        >
          Delete
        </v-btn>
      </div>
    </div>

    <v-alert v-if="error" type="error" class="mb-4" closable>
      {{ error }}
    </v-alert>

    <v-alert
      v-if="fixNotice"
      type="info"
      variant="tonal"
      class="mb-4"
      closable
      @click:close="fixNotice = null"
    >
      Queued a fix with
      <strong>{{ fixNotice.agent === "claude" ? "Claude" : "Codex" }}</strong>.
      The backend endpoint that clones the repo and runs the agent is not
      wired up yet — this notice confirms the button worked end-to-end.
    </v-alert>

    <v-progress-linear v-if="loading" indeterminate color="primary" />

    <template v-if="detail">
      <v-card class="mb-4">
        <v-card-title class="text-subtitle-1">Description</v-card-title>
        <v-card-text>
          <div v-if="detail.description" class="text-body-2">
            {{ detail.description }}
          </div>
          <div v-else class="text-body-2 text-secondary font-italic">
            No description provided.
          </div>
          <div class="mt-3 d-flex ga-2 flex-wrap">
            <v-chip variant="outlined" size="small" color="secondary">
              Created {{ formatAge(detail.created_at, { suffix: " ago" }) }}
            </v-chip>
            <v-chip
              v-if="detail.updated_at"
              variant="outlined"
              size="small"
              color="secondary"
            >
              Updated {{ formatAge(detail.updated_at, { suffix: " ago" }) }}
            </v-chip>
          </div>
        </v-card-text>
      </v-card>

      <!-- Aggregated resource usage across all member deployments -->
      <v-card class="mb-4 pa-4">
        <div class="d-flex align-center mb-2">
          <v-icon icon="mdi-chart-line" class="mr-2" />
          <span class="text-subtitle-1">Resource Usage</span>
          <v-spacer />
          <span class="text-caption text-secondary">
            {{ memberDeploymentNames.length }} deployment(s) &middot; metrics-server
          </span>
        </div>
        <v-alert
          v-if="metricsUnavailable"
          type="info"
          density="compact"
          variant="tonal"
        >
          {{ metricsUnavailable }}
        </v-alert>
        <div v-else class="d-flex flex-wrap ga-6 align-center">
          <MetricSparkline
            label="CPU"
            :data="appCpuSeries"
            :latest-formatted="formatCpuTotal(appPodCpu)"
            color="primary"
            :width="140"
            :height="28"
            fill
          />
          <MetricSparkline
            label="Memory"
            :data="appMemSeries"
            :latest-formatted="formatMemTotal(appPodMem)"
            color="secondary"
            :width="140"
            :height="28"
            fill
          />
        </div>
      </v-card>

      <v-card v-if="detail.git" class="mb-4">
        <v-card-title class="d-flex align-center">
          <v-icon icon="mdi-source-branch" class="mr-2" />
          <span class="text-subtitle-1">Git Repository</span>
        </v-card-title>
        <v-card-text>
          <div class="d-flex flex-column ga-2">
            <div>
              <span class="text-caption text-secondary mr-2">Repo:</span>
              <a
                :href="detail.git.repo_url"
                target="_blank"
                rel="noopener"
                class="text-body-2"
              >
                {{ detail.git.repo_url }}
              </a>
            </div>
            <div>
              <span class="text-caption text-secondary mr-2">Branch:</span>
              <code class="text-body-2">{{ detail.git.branch ?? "main" }}</code>
            </div>
            <div v-if="detail.git.token_secret">
              <span class="text-caption text-secondary mr-2">Token secret:</span>
              <code class="text-body-2">{{ detail.git.token_secret }}</code>
            </div>
          </div>
        </v-card-text>
      </v-card>

      <v-card class="mb-4">
        <v-card-title class="text-subtitle-1">
          Deployments ({{ detail.deployments.length }})
        </v-card-title>
        <v-data-table
          :items="detail.deployments"
          :headers="deploymentHeaders"
          item-value="name"
          density="compact"
          hover
          hide-default-footer
          @click:row="(_: any, row: any) => goToDeployment(row.item.name)"
        >
          <template v-slot:item.name="{ item }">
            <span class="font-weight-medium">{{ item.name }}</span>
          </template>
          <template v-slot:item.image="{ item }">
            <span class="text-caption text-secondary">{{ item.image }}</span>
          </template>
          <template v-slot:item.status="{ item }">
            <StatusChip :status="item.status" />
          </template>
          <template v-slot:item.replicas="{ item }">
            <span class="text-body-2">
              {{ item.replicas.ready }} / {{ item.replicas.desired }}
            </span>
          </template>
          <template v-slot:item.actions="{ item }">
            <v-btn
              icon="mdi-link-off"
              size="x-small"
              variant="text"
              color="error"
              @click.stop="handleRemoveMember('Deployment', item.name)"
            />
          </template>
          <template v-slot:no-data>
            <div class="text-center py-4 text-secondary text-body-2">
              No deployments attached
            </div>
          </template>
        </v-data-table>
      </v-card>

      <v-card class="mb-4">
        <v-card-title class="text-subtitle-1">
          CronJobs ({{ detail.cronjobs.length }})
        </v-card-title>
        <v-data-table
          :items="detail.cronjobs"
          :headers="cronjobHeaders"
          item-value="name"
          density="compact"
          hide-default-footer
        >
          <template v-slot:item.name="{ item }">
            <span class="font-weight-medium">{{ item.name }}</span>
          </template>
          <template v-slot:item.schedule="{ item }">
            <code class="text-body-2">{{ item.schedule }}</code>
          </template>
          <template v-slot:item.suspend="{ item }">
            <v-chip
              :color="item.suspend ? 'warning' : 'success'"
              size="x-small"
              variant="tonal"
            >
              {{ item.suspend ? "Yes" : "No" }}
            </v-chip>
          </template>
          <template v-slot:item.active_count="{ item }">
            <span class="text-body-2">{{ item.active_count }}</span>
          </template>
          <template v-slot:item.actions="{ item }">
            <v-btn
              icon="mdi-link-off"
              size="x-small"
              variant="text"
              color="error"
              @click.stop="handleRemoveMember('CronJob', item.name)"
            />
          </template>
          <template v-slot:no-data>
            <div class="text-center py-4 text-secondary text-body-2">
              No cronjobs attached
            </div>
          </template>
        </v-data-table>
      </v-card>
    </template>

    <AddMemberDialog
      v-model="showAddMemberDialog"
      :namespace="namespace"
      :application-name="name"
      @added="onMemberAdded"
    />

    <v-dialog v-model="showDeleteDialog" max-width="480">
      <v-card>
        <v-card-title>Delete Application</v-card-title>
        <v-card-text>
          <p class="mb-3">
            Are you sure you want to delete application
            <strong>{{ name }}</strong>?
          </p>
          <v-checkbox
            v-model="cascadeDelete"
            label="Also delete all member resources (deployments and cronjobs)"
            density="compact"
            hide-details
            color="error"
          />
          <v-alert
            v-if="cascadeDelete"
            type="warning"
            density="compact"
            variant="tonal"
            class="mt-3"
          >
            This will delete all attached deployments and cronjobs. This cannot be undone.
          </v-alert>
          <div class="mt-4 text-body-2">
            Type <code>{{ name }}</code> to confirm:
          </div>
          <v-text-field
            v-model="typedDeleteConfirm"
            variant="outlined"
            density="compact"
            hide-details
            autofocus
            class="mt-2"
          />
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="showDeleteDialog = false">Cancel</v-btn>
          <v-btn
            color="error"
            variant="flat"
            :loading="actionLoading"
            :disabled="!deleteConfirmMatches"
            @click="handleDelete"
          >
            Delete
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>

    <v-dialog v-model="showEditDialog" max-width="560">
      <v-card>
        <v-card-title>Edit Application</v-card-title>
        <v-card-text>
          <v-textarea
            v-model="editDescription"
            label="Description"
            rows="3"
            variant="outlined"
            density="compact"
            class="mb-3"
          />
          <v-switch
            v-model="editHasGit"
            label="Connect a Git repository"
            color="primary"
            density="compact"
            hide-details
            class="mb-3"
          />
          <template v-if="editHasGit">
            <v-text-field
              v-model="editGit.repo_url"
              label="Repository URL"
              variant="outlined"
              density="compact"
              class="mb-2"
            />
            <v-text-field
              v-model="editGit.branch"
              label="Branch"
              placeholder="main"
              variant="outlined"
              density="compact"
              class="mb-2"
            />
            <v-text-field
              v-model="editGit.token_secret"
              label="Access token secret name"
              hint="Name of the Kubernetes Secret holding the Git access token"
              persistent-hint
              variant="outlined"
              density="compact"
            />
          </template>
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="showEditDialog = false">Cancel</v-btn>
          <v-btn
            color="primary"
            variant="flat"
            :loading="actionLoading"
            @click="handleSaveEdit"
          >
            Save
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>
