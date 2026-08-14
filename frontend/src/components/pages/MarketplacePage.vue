<template>
  <v-container fluid class="pa-4">
    <v-row>
      <v-col>
        <div class="d-flex align-center mb-4">
          <v-icon class="mr-2" color="primary">mdi-store</v-icon>
          <span class="text-h5 font-weight-bold">Plugin Marketplace</span>
          <v-spacer />
          <v-btn icon="mdi-refresh" variant="text" :loading="loading" @click="load" />
        </div>

        <!-- Disabled -->
        <v-alert v-if="!marketplaceUrl" type="info" variant="tonal" class="mb-4">
          Marketplace is disabled. Set a catalog URL in
          <router-link to="/settings">Settings → Advanced</router-link> to enable.
        </v-alert>

        <!-- Error -->
        <v-alert v-else-if="error" type="error" variant="tonal" class="mb-4">
          {{ error }}
        </v-alert>

        <template v-else>
          <!-- Search + filter -->
          <v-row class="mb-3">
            <v-col cols="12" sm="6">
              <v-text-field
                v-model="search"
                prepend-inner-icon="mdi-magnify"
                placeholder="Search plugins..."
                variant="outlined"
                density="compact"
                hide-details
                clearable
              />
            </v-col>
            <v-col cols="12" sm="6" class="d-flex flex-wrap gap-2 align-center">
              <v-chip
                v-for="tag in popularTags"
                :key="tag"
                :color="activeTag === tag ? 'primary' : undefined"
                :variant="activeTag === tag ? 'elevated' : 'outlined'"
                size="small"
                clickable
                @click="activeTag = activeTag === tag ? null : tag"
              >
                {{ tag }}
              </v-chip>
            </v-col>
          </v-row>

          <!-- Loading skeletons -->
          <v-row v-if="loading">
            <v-col v-for="n in 6" :key="n" cols="12" sm="6" md="4">
              <v-skeleton-loader type="card" />
            </v-col>
          </v-row>

          <!-- Plugin cards -->
          <v-row v-else>
            <v-col
              v-for="plugin in filtered"
              :key="plugin.slug"
              cols="12"
              sm="6"
              md="4"
            >
              <v-card height="100%" class="d-flex flex-column">
                <v-card-title class="d-flex align-center pb-1">
                  <span class="text-body-1 font-weight-bold">{{ plugin.name }}</span>
                  <v-spacer />
                  <v-chip
                    :color="plugin.trust_level === 'verified' ? 'success' : 'warning'"
                    size="x-small"
                    variant="elevated"
                    class="ml-2"
                  >
                    {{ plugin.trust_level }}
                  </v-chip>
                </v-card-title>

                <v-card-text class="flex-grow-1">
                  <p class="text-body-2 text-medium-emphasis mb-3">{{ plugin.description }}</p>
                  <div class="d-flex flex-wrap gap-1">
                    <v-chip
                      v-for="tag in plugin.tags"
                      :key="tag"
                      size="x-small"
                      variant="tonal"
                    >
                      {{ tag }}
                    </v-chip>
                  </div>
                </v-card-text>

                <v-card-actions>
                  <span class="text-caption text-medium-emphasis ml-2">v{{ plugin.latest_version }}</span>
                  <v-spacer />
                  <v-btn
                    :href="plugin.homepage"
                    target="_blank"
                    variant="text"
                    size="small"
                    icon="mdi-open-in-new"
                  />
                  <v-chip
                    v-if="isInstalled(plugin)"
                    color="success"
                    size="small"
                    variant="tonal"
                  >
                    Installed
                  </v-chip>
                  <v-btn
                    v-else
                    size="small"
                    variant="tonal"
                    @click="openInstall(plugin)"
                  >
                    Install
                  </v-btn>
                </v-card-actions>
              </v-card>
            </v-col>

            <v-col v-if="filtered.length === 0" cols="12">
              <v-empty-state
                icon="mdi-magnify"
                title="No plugins found"
                text="Try a different search or filter."
              />
            </v-col>
          </v-row>
        </template>
      </v-col>
    </v-row>

    <!-- Install dialog -->
    <v-dialog v-model="installDialog" max-width="560">
      <v-card v-if="installing">
        <v-card-title>Install {{ installing.name }}</v-card-title>
        <v-card-text>
          <v-alert
            v-if="installing.trust_level === 'community'"
            type="warning"
            variant="tonal"
            class="mb-4"
          >
            This plugin has not been reviewed by the deckwatch project. Review the source repository before installing.
          </v-alert>

          <p class="text-body-2 mb-3">{{ installing.description }}</p>

          <v-table density="compact" class="mb-2">
            <tbody>
              <tr><td class="text-medium-emphasis">Source</td><td>{{ pluginSourceLabel(installing) }}</td></tr>
              <tr><td class="text-medium-emphasis">Version</td><td>{{ installing.latest_version }}</td></tr>
              <tr v-if="installing.allowed_hosts_hint?.length"><td class="text-medium-emphasis">Allowed hosts</td><td>{{ installing.allowed_hosts_hint.join(', ') }}</td></tr>
            </tbody>
          </v-table>
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="installDialog = false">Cancel</v-btn>
          <v-btn color="primary" variant="elevated" @click="confirmInstall">Install</v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </v-container>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import type { MarketplaceCatalog, MarketplaceEntry, PluginSummary, PluginSourceGithub } from '@/types/api'

const loading = ref(false)
const error = ref<string | null>(null)
const catalog = ref<MarketplaceCatalog | null>(null)
const installedPlugins = ref<PluginSummary[]>([])
const search = ref('')
const activeTag = ref<string | null>(null)
const installDialog = ref(false)
const installing = ref<MarketplaceEntry | null>(null)
const marketplaceUrl = ref('')

async function load() {
  // Read marketplace_url from settings
  try {
    const settingsRes = await fetch('/api/settings')
    const settings = await settingsRes.json()
    marketplaceUrl.value = settings.marketplace_url ?? ''
    if (!marketplaceUrl.value) return

    loading.value = true
    error.value = null

    const [catalogRes, pluginsRes] = await Promise.all([
      fetch(marketplaceUrl.value),
      fetch('/api/plugins'),
    ])

    if (!catalogRes.ok) throw new Error(`Failed to fetch catalog: ${catalogRes.status}`)

    catalog.value = await catalogRes.json()
    installedPlugins.value = await pluginsRes.json()
  } catch (e: any) {
    error.value = e.message ?? 'Failed to load marketplace'
  } finally {
    loading.value = false
  }
}

const popularTags = computed(() => {
  if (!catalog.value) return []
  const counts: Record<string, number> = {}
  for (const p of catalog.value.plugins) {
    for (const t of p.tags) counts[t] = (counts[t] ?? 0) + 1
  }
  return Object.entries(counts).sort((a, b) => b[1] - a[1]).slice(0, 8).map(([t]) => t)
})

const filtered = computed(() => {
  if (!catalog.value) return []
  return catalog.value.plugins.filter(p => {
    const q = search.value.toLowerCase()
    const matchesSearch = !q || p.name.toLowerCase().includes(q) || p.description.toLowerCase().includes(q) || p.tags.some(t => t.includes(q))
    const matchesTag = !activeTag.value || p.tags.includes(activeTag.value)
    return matchesSearch && matchesTag
  })
})

function isInstalled(plugin: MarketplaceEntry) {
  return installedPlugins.value.some(p =>
    p.name.toLowerCase() === plugin.slug.replace('deckwatch-plugin-', '').toLowerCase() ||
    p.name.toLowerCase() === plugin.name.toLowerCase()
  )
}

function openInstall(plugin: MarketplaceEntry) {
  installing.value = plugin
  installDialog.value = true
}

function pluginSourceLabel(entry: MarketplaceEntry): string {
  if (entry.source.type === 'github') {
    const s = entry.source as PluginSourceGithub;
    return `${s.repo} @ ${s.ref}`;
  }
  return entry.source.url;
}

async function confirmInstall() {
  if (!installing.value) return
  try {
    const settingsRes = await fetch('/api/settings')
    const settings = await settingsRes.json()
    const p = installing.value
    settings.plugins = settings.plugins ?? []
    settings.plugins.push({
      name: p.slug.replace('deckwatch-plugin-', ''),
      enabled: true,
      source: p.source,
      allowed_hosts: p.allowed_hosts_hint ?? [],
      config: {},
      inherit_env_keys: [],
      inherit_env_file_keys: {},
    })
    await fetch('/api/settings', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(settings),
    })
    installDialog.value = false
    await load()
  } catch (e: any) {
    error.value = `Install failed: ${e.message}`
  }
}

onMounted(load)
</script>
