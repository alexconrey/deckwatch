<script setup lang="ts">
// Browsable UI for the embedded deckwatch OCI registry.
//
// Layout: two-pane. Left = repository list (filterable). Right = tag list
// for the currently selected repo, plus a manifest-detail expander on
// click. Delete is behind a confirm dialog because there's no soft-delete
// or restore path once a tag file is removed.

import { computed, onMounted, ref, watch } from "vue";
import { registryApi } from "@/api/registry";
import type {
  ManifestDetail,
  RepositorySummary,
  TagSummary,
} from "@/api/registry";
import { ApiError } from "@/api/client";

const repos = ref<RepositorySummary[]>([]);
const reposLoading = ref(false);
const reposError = ref<string | null>(null);

const selectedRepo = ref<string | null>(null);
const tags = ref<TagSummary[]>([]);
const tagsLoading = ref(false);
const tagsError = ref<string | null>(null);

const repoFilter = ref("");

const showManifest = ref(false);
const manifest = ref<ManifestDetail | null>(null);
const manifestLoading = ref(false);
const manifestError = ref<string | null>(null);

const showDeleteConfirm = ref(false);
const pendingDelete = ref<{ name: string; tag: string } | null>(null);
const deleting = ref(false);

const registryDisabled = ref(false);

const filteredRepos = computed(() => {
  if (!repoFilter.value) return repos.value;
  const needle = repoFilter.value.toLowerCase();
  return repos.value.filter((r) => r.name.toLowerCase().includes(needle));
});

const humanBytes = (n: number): string => {
  if (n === 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), units.length - 1);
  const v = n / Math.pow(1024, i);
  return `${v.toFixed(v >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
};

const shortDigest = (d: string): string => {
  const hex = d.replace(/^sha256:/, "");
  return `sha256:${hex.slice(0, 12)}`;
};

const relativeAge = (iso: string | null): string => {
  if (!iso) return "-";
  const diff = Date.now() - new Date(iso).getTime();
  const min = Math.floor(diff / 60000);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const d = Math.floor(hr / 24);
  return `${d}d ago`;
};

const fetchRepos = async () => {
  reposLoading.value = true;
  reposError.value = null;
  try {
    const res = await registryApi.listRepositories();
    repos.value = res.repositories;
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) {
      registryDisabled.value = true;
    } else {
      reposError.value = err instanceof Error ? err.message : String(err);
    }
  } finally {
    reposLoading.value = false;
  }
};

const fetchTags = async (name: string) => {
  tagsLoading.value = true;
  tagsError.value = null;
  try {
    const res = await registryApi.listTags(name);
    tags.value = res.tags;
  } catch (err) {
    tagsError.value = err instanceof Error ? err.message : String(err);
  } finally {
    tagsLoading.value = false;
  }
};

const selectRepo = (name: string) => {
  selectedRepo.value = name;
};

const openManifest = async (tag: TagSummary) => {
  if (!selectedRepo.value) return;
  showManifest.value = true;
  manifest.value = null;
  manifestError.value = null;
  manifestLoading.value = true;
  try {
    manifest.value = await registryApi.getManifest(selectedRepo.value, tag.tag);
  } catch (err) {
    manifestError.value = err instanceof Error ? err.message : String(err);
  } finally {
    manifestLoading.value = false;
  }
};

const askDelete = (tag: TagSummary) => {
  if (!selectedRepo.value) return;
  pendingDelete.value = { name: selectedRepo.value, tag: tag.tag };
  showDeleteConfirm.value = true;
};

const confirmDelete = async () => {
  if (!pendingDelete.value) return;
  deleting.value = true;
  try {
    await registryApi.deleteTag(pendingDelete.value.name, pendingDelete.value.tag);
    showDeleteConfirm.value = false;
    // Refresh both panes — a repo with zero tags shouldn't linger in the
    // list because the backend filters empty dirs out of _catalog.
    await Promise.all([fetchRepos(), fetchTags(pendingDelete.value.name)]);
    pendingDelete.value = null;
  } catch (err) {
    tagsError.value = err instanceof Error ? err.message : String(err);
  } finally {
    deleting.value = false;
  }
};

watch(selectedRepo, (name) => {
  if (name) {
    fetchTags(name);
  } else {
    tags.value = [];
  }
});

onMounted(fetchRepos);

const tagHeaders = [
  { title: "Tag", key: "tag" },
  { title: "Digest", key: "digest", width: "260px" },
  { title: "Size", key: "size", width: "110px" },
  { title: "Pushed", key: "created", width: "120px" },
  { title: "", key: "actions", width: "160px", sortable: false },
];
</script>

<template>
  <div v-if="registryDisabled" class="text-center pa-8">
    <v-icon icon="mdi-package-variant-closed-remove" size="80" color="grey" class="mb-4" />
    <div class="text-h5 mb-2">Registry not enabled</div>
    <div class="text-body-1 text-medium-emphasis mb-4">
      The embedded OCI registry is disabled on this deckwatch instance.
      Set <code>registry.enabled=true</code> in your Helm values and redeploy
      to turn it on.
    </div>
    <v-btn
      variant="outlined"
      href="https://github.com/alexconrey/deckwatch/blob/main/docs/REGISTRY.md"
      target="_blank"
    >
      Read the registry docs
    </v-btn>
  </div>

  <v-row v-else no-gutters class="registry-page">
    <v-col cols="12" md="4" lg="3" class="pr-md-2">
      <v-card variant="outlined" height="100%">
        <v-card-title class="d-flex align-center">
          <span>Repositories</span>
          <v-spacer />
          <v-btn
            icon="mdi-refresh"
            size="small"
            variant="text"
            :loading="reposLoading"
            @click="fetchRepos"
          />
        </v-card-title>
        <v-card-text>
          <v-text-field
            v-model="repoFilter"
            density="compact"
            variant="outlined"
            prepend-inner-icon="mdi-magnify"
            placeholder="Filter repositories"
            hide-details
            clearable
            class="mb-3"
          />
          <v-alert v-if="reposError" type="error" density="compact" class="mb-3">
            {{ reposError }}
          </v-alert>
          <v-list
            v-if="filteredRepos.length"
            density="compact"
            nav
            :selected="selectedRepo ? [selectedRepo] : []"
          >
            <v-list-item
              v-for="r in filteredRepos"
              :key="r.name"
              :value="r.name"
              :title="r.name"
              :subtitle="`${r.tag_count} tag${r.tag_count === 1 ? '' : 's'} · ${humanBytes(r.total_size)}`"
              prepend-icon="mdi-package-variant"
              @click="selectRepo(r.name)"
            />
          </v-list>
          <div
            v-else-if="!reposLoading"
            class="text-center text-medium-emphasis py-8"
          >
            <v-icon icon="mdi-package-variant-closed" size="48" class="mb-2" />
            <div>No images pushed yet</div>
            <div class="text-caption mt-1">
              Trigger a GitOps build to populate this registry.
            </div>
          </div>
        </v-card-text>
      </v-card>
    </v-col>

    <v-col cols="12" md="8" lg="9" class="pl-md-2">
      <v-card variant="outlined" height="100%">
        <v-card-title>
          <template v-if="selectedRepo">
            <v-icon icon="mdi-tag-multiple" class="mr-2" />
            {{ selectedRepo }}
          </template>
          <template v-else>
            Select a repository
          </template>
        </v-card-title>
        <v-card-text>
          <template v-if="!selectedRepo">
            <div class="text-center text-medium-emphasis py-8">
              Pick a repository from the list on the left to browse its tags.
            </div>
          </template>
          <template v-else>
            <v-alert v-if="tagsError" type="error" density="compact" class="mb-3">
              {{ tagsError }}
            </v-alert>
            <v-data-table
              :headers="tagHeaders"
              :items="tags"
              :loading="tagsLoading"
              density="comfortable"
              items-per-page="25"
            >
              <template #[`item.digest`]="{ item }">
                <code class="text-caption">{{ shortDigest(item.digest) }}</code>
              </template>
              <template #[`item.size`]="{ item }">
                {{ humanBytes(item.size) }}
              </template>
              <template #[`item.created`]="{ item }">
                {{ relativeAge(item.created) }}
              </template>
              <template #[`item.actions`]="{ item }">
                <v-btn
                  size="x-small"
                  variant="text"
                  icon="mdi-file-document-outline"
                  @click="openManifest(item)"
                />
                <v-btn
                  size="x-small"
                  variant="text"
                  icon="mdi-delete-outline"
                  color="error"
                  @click="askDelete(item)"
                />
              </template>
            </v-data-table>
          </template>
        </v-card-text>
      </v-card>
    </v-col>
  </v-row>

  <v-dialog v-model="showManifest" max-width="900">
    <v-card>
      <v-card-title>
        <v-icon icon="mdi-file-document-outline" class="mr-2" />
        Manifest
        <span v-if="manifest" class="text-body-2 text-medium-emphasis ml-2">
          {{ manifest.name }}:{{ manifest.tag }}
        </span>
      </v-card-title>
      <v-card-text>
        <v-progress-linear v-if="manifestLoading" indeterminate />
        <v-alert v-if="manifestError" type="error" density="compact">
          {{ manifestError }}
        </v-alert>
        <template v-if="manifest">
          <v-list density="compact">
            <v-list-item>
              <v-list-item-title>Digest</v-list-item-title>
              <v-list-item-subtitle>
                <code>{{ manifest.digest }}</code>
              </v-list-item-subtitle>
            </v-list-item>
            <v-list-item>
              <v-list-item-title>Media Type</v-list-item-title>
              <v-list-item-subtitle>{{ manifest.media_type }}</v-list-item-subtitle>
            </v-list-item>
            <v-list-item>
              <v-list-item-title>Total Size</v-list-item-title>
              <v-list-item-subtitle>{{ humanBytes(manifest.total_size) }}</v-list-item-subtitle>
            </v-list-item>
          </v-list>
          <v-divider class="my-3" />
          <div class="text-subtitle-2 mb-2" v-if="manifest.config">
            Config Blob
          </div>
          <div v-if="manifest.config" class="mb-4">
            <code class="text-caption">{{ shortDigest(manifest.config.digest) }}</code>
            <span class="ml-2 text-medium-emphasis">{{ humanBytes(manifest.config.size) }}</span>
          </div>
          <div class="text-subtitle-2 mb-2">Layers ({{ manifest.layers.length }})</div>
          <v-table density="compact">
            <thead>
              <tr>
                <th>Digest</th>
                <th>Size</th>
                <th>Media Type</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="l in manifest.layers" :key="l.digest">
                <td><code class="text-caption">{{ shortDigest(l.digest) }}</code></td>
                <td>{{ humanBytes(l.size) }}</td>
                <td class="text-caption">{{ l.media_type }}</td>
              </tr>
            </tbody>
          </v-table>
        </template>
      </v-card-text>
      <v-card-actions>
        <v-spacer />
        <v-btn @click="showManifest = false">Close</v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>

  <v-dialog v-model="showDeleteConfirm" max-width="500">
    <v-card>
      <v-card-title>Delete tag?</v-card-title>
      <v-card-text v-if="pendingDelete">
        This will remove <code>{{ pendingDelete.name }}:{{ pendingDelete.tag }}</code>
        from the registry. Blobs shared with other tags are kept; unreferenced
        layers are not garbage-collected.
      </v-card-text>
      <v-card-actions>
        <v-spacer />
        <v-btn @click="showDeleteConfirm = false" :disabled="deleting">Cancel</v-btn>
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
</template>

<style scoped>
.registry-page {
  min-height: calc(100vh - 200px);
}
</style>
