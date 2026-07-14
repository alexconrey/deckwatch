<script setup lang="ts">
// Two-pane picker for choosing a container image from the embedded deckwatch
// OCI registry. Emits a fully-qualified reference (registry-url/repo:tag) so
// callers can drop the string straight into an image field without having
// to know the registry hostname.
//
// The registry URL is discovered from the settings API: the backend injects
// a synthetic OciRegistry with `builtin: true` and `registry_type: "deckwatch"`
// pointing at the public URL of the embedded registry. If that entry is
// missing the registry is disabled and the picker refuses to open.

import { computed, ref, watch } from "vue";
import { registryApi } from "@/api/registry";
import type { RepositorySummary, TagSummary } from "@/api/registry";
import { settingsApi } from "@/api/settings";
import { ApiError } from "@/api/client";
import { formatAge } from "@/utils/format";

const props = defineProps<{
  modelValue: boolean;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  selected: [imageRef: string];
}>();

const registryUrl = ref<string | null>(null);
const registryEnabled = ref<boolean | null>(null);
const capabilityError = ref<string | null>(null);

const repos = ref<RepositorySummary[]>([]);
const reposLoading = ref(false);
const reposError = ref<string | null>(null);
const repoFilter = ref("");

const selectedRepo = ref<string | null>(null);
const tags = ref<TagSummary[]>([]);
const tagsLoading = ref(false);
const tagsError = ref<string | null>(null);
const tagFilter = ref("");

const filteredRepos = computed(() => {
  if (!repoFilter.value) return repos.value;
  const needle = repoFilter.value.toLowerCase();
  return repos.value.filter((r) => r.name.toLowerCase().includes(needle));
});

const filteredTags = computed(() => {
  if (!tagFilter.value) return tags.value;
  const needle = tagFilter.value.toLowerCase();
  return tags.value.filter(
    (t) =>
      t.tag.toLowerCase().includes(needle) ||
      t.digest.toLowerCase().includes(needle),
  );
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

const relativeAge = (iso: string | null): string =>
  formatAge(iso, { suffix: " ago" });

// Strip protocol so `registry.example.com/repo:tag` looks like a normal
// image reference. Callers pasting into a Kubernetes image field don't want
// `https://` prefixes — kubelet expects a bare host or host:port.
const normalizeRegistryHost = (url: string): string =>
  url.replace(/^https?:\/\//, "").replace(/\/+$/, "");

async function loadCapability() {
  capabilityError.value = null;
  try {
    const cap = await registryApi.enabled();
    registryEnabled.value = cap.enabled;
    if (!cap.enabled) return;

    // Registry URL isn't on /registry/enabled — the backend surfaces it via
    // the settings API as a synthetic OciRegistry entry.
    const settings = await settingsApi.get();
    const builtin = (settings.oci_registries ?? []).find(
      (r) => r.builtin && r.registry_type === "deckwatch",
    );
    if (builtin) {
      registryUrl.value = normalizeRegistryHost(builtin.url);
    } else {
      // Registry is on but no public URL configured — the operator can still
      // browse tags, but we can't build a pullable reference. Fall back to a
      // bare `repo:tag` and let the user prepend their own host if needed.
      registryUrl.value = null;
    }
  } catch (err) {
    capabilityError.value =
      err instanceof Error ? err.message : String(err);
  }
}

async function fetchRepos() {
  reposLoading.value = true;
  reposError.value = null;
  try {
    const res = await registryApi.listRepositories();
    repos.value = res.repositories;
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) {
      registryEnabled.value = false;
    } else {
      reposError.value = err instanceof Error ? err.message : String(err);
    }
  } finally {
    reposLoading.value = false;
  }
}

async function fetchTags(name: string) {
  tagsLoading.value = true;
  tagsError.value = null;
  try {
    const res = await registryApi.listTags(name);
    // Newest tags first — operators usually want the latest push, and the
    // backend returns them in filesystem order which is effectively random.
    tags.value = [...res.tags].sort((a, b) => {
      const ta = a.created ? new Date(a.created).getTime() : 0;
      const tb = b.created ? new Date(b.created).getTime() : 0;
      return tb - ta;
    });
  } catch (err) {
    tagsError.value = err instanceof Error ? err.message : String(err);
  } finally {
    tagsLoading.value = false;
  }
}

function selectRepo(name: string) {
  selectedRepo.value = name;
  tagFilter.value = "";
}

function pickTag(tag: TagSummary) {
  if (!selectedRepo.value) return;
  const ref = registryUrl.value
    ? `${registryUrl.value}/${selectedRepo.value}:${tag.tag}`
    : `${selectedRepo.value}:${tag.tag}`;
  emit("selected", ref);
  emit("update:modelValue", false);
}

function close() {
  emit("update:modelValue", false);
}

// Reload each time the dialog is opened — tag lists can change under us
// between opens (a GitOps build may have pushed a new tag). Reset selection
// state so the user starts from the repo list rather than a stale tag pane.
watch(
  () => props.modelValue,
  async (open) => {
    if (!open) return;
    repoFilter.value = "";
    tagFilter.value = "";
    selectedRepo.value = null;
    tags.value = [];
    await loadCapability();
    if (registryEnabled.value) {
      await fetchRepos();
    }
  },
);

watch(selectedRepo, (name) => {
  if (name) {
    fetchTags(name);
  } else {
    tags.value = [];
  }
});

const tagHeaders = [
  { title: "Tag", key: "tag" },
  { title: "Digest", key: "digest", width: "200px" },
  { title: "Size", key: "size", width: "100px" },
  { title: "Age", key: "created", width: "110px" },
];
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="1100"
    scrollable
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card height="720">
      <v-card-title class="d-flex align-center">
        <v-icon icon="mdi-image-search" class="mr-2" />
        Browse Registry
        <span
          v-if="registryUrl"
          class="text-caption text-medium-emphasis ml-3"
        >
          {{ registryUrl }}
        </span>
        <v-spacer />
        <v-btn
          icon="mdi-close"
          size="small"
          variant="text"
          @click="close"
        />
      </v-card-title>

      <v-divider />

      <v-card-text class="pa-0" style="overflow: hidden">
        <v-alert
          v-if="capabilityError"
          type="error"
          density="compact"
          class="ma-4"
        >
          {{ capabilityError }}
        </v-alert>

        <div
          v-else-if="registryEnabled === false"
          class="text-center pa-8"
        >
          <v-icon
            icon="mdi-package-variant-closed-remove"
            size="72"
            color="grey"
            class="mb-3"
          />
          <div class="text-h6 mb-1">Registry not enabled</div>
          <div class="text-body-2 text-medium-emphasis">
            The embedded OCI registry is disabled on this deckwatch instance.
            Set <code>registry.enabled=true</code> in your Helm values to
            enable browsing.
          </div>
        </div>

        <v-row
          v-else
          no-gutters
          class="fill-height"
          style="height: 620px"
        >
          <!-- Left pane: repositories -->
          <v-col
            cols="4"
            class="d-flex flex-column"
            style="border-right: 1px solid rgba(0, 0, 0, 0.12)"
          >
            <div class="pa-3">
              <v-text-field
                v-model="repoFilter"
                density="compact"
                variant="outlined"
                prepend-inner-icon="mdi-magnify"
                placeholder="Filter repositories"
                hide-details
                clearable
              />
            </div>
            <v-alert
              v-if="reposError"
              type="error"
              density="compact"
              class="mx-3 mb-2"
            >
              {{ reposError }}
            </v-alert>
            <div style="overflow-y: auto; flex: 1">
              <v-progress-linear
                v-if="reposLoading"
                indeterminate
                color="primary"
              />
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
                class="text-center text-medium-emphasis pa-6"
              >
                <v-icon
                  icon="mdi-package-variant-closed"
                  size="40"
                  class="mb-2"
                />
                <div class="text-body-2">
                  {{ repoFilter ? "No matching repositories" : "No images pushed yet" }}
                </div>
              </div>
            </div>
          </v-col>

          <!-- Right pane: tags for the selected repo -->
          <v-col cols="8" class="d-flex flex-column">
            <template v-if="!selectedRepo">
              <div class="text-center text-medium-emphasis pa-12 flex-grow-1 d-flex flex-column justify-center align-center">
                <v-icon icon="mdi-tag-multiple-outline" size="56" class="mb-3" />
                <div>Pick a repository on the left to see its tags.</div>
              </div>
            </template>
            <template v-else>
              <div class="pa-3 d-flex align-center ga-2">
                <v-icon icon="mdi-tag-multiple" />
                <span class="text-subtitle-1 font-weight-medium">
                  {{ selectedRepo }}
                </span>
                <v-spacer />
                <v-text-field
                  v-model="tagFilter"
                  density="compact"
                  variant="outlined"
                  prepend-inner-icon="mdi-magnify"
                  placeholder="Filter tags"
                  hide-details
                  clearable
                  style="max-width: 260px"
                />
              </div>
              <v-alert
                v-if="tagsError"
                type="error"
                density="compact"
                class="mx-3 mb-2"
              >
                {{ tagsError }}
              </v-alert>
              <div style="overflow-y: auto; flex: 1" class="px-3 pb-3">
                <v-data-table
                  :headers="tagHeaders"
                  :items="filteredTags"
                  :loading="tagsLoading"
                  density="comfortable"
                  items-per-page="50"
                  hover
                  @click:row="(_e: unknown, ctx: { item: TagSummary }) => pickTag(ctx.item)"
                >
                  <template #[`item.tag`]="{ item }">
                    <span class="font-weight-medium">{{ item.tag }}</span>
                  </template>
                  <template #[`item.digest`]="{ item }">
                    <code class="text-caption">{{ shortDigest(item.digest) }}</code>
                  </template>
                  <template #[`item.size`]="{ item }">
                    {{ humanBytes(item.size) }}
                  </template>
                  <template #[`item.created`]="{ item }">
                    {{ relativeAge(item.created) }}
                  </template>
                </v-data-table>
              </div>
            </template>
          </v-col>
        </v-row>
      </v-card-text>

      <v-divider />

      <v-card-actions>
        <div class="text-caption text-medium-emphasis pl-2">
          Click a tag to select it.
        </div>
        <v-spacer />
        <v-btn variant="text" @click="close">Cancel</v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>
