<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { useTheme } from "vuetify";
import { useNamespaceStore } from "@/stores/namespace";
import { useAuth } from "@/composables/useAuth";
import { useFeatures } from "@/composables/useFeatures";
import CreateNamespaceDialog from "@/components/common/CreateNamespaceDialog.vue";

const router = useRouter();
const ns = useNamespaceStore();
const auth = useAuth();
const { features } = useFeatures();
const theme = useTheme();
const showCreateNs = ref(false);

const isDark = computed(() => theme.global.current.value.dark);
const toggleTheme = () => {
  const next = isDark.value ? "deckwatchLight" : "deckwatchDark";
  theme.global.name.value = next;
  localStorage.setItem("deckwatch-theme", next);
};

// Sentinel value appended to the namespace dropdown items list so a
// "Create Namespace" action lives inside the same v-select — avoids a
// separate "+" button next to the field. Chosen to be a string that can
// never collide with a real namespace name (kubernetes forbids "__").
const CREATE_SENTINEL = "__create_namespace__";

onMounted(() => {
  ns.fetchNamespaces();
});

// v-select needs its items to be a plain array of strings-or-objects.
// Mixing strings (real namespaces) with an object item works because
// v-select accepts either — the object form gives us title/value/props
// so we can style the create row differently.
const nsItems = computed(() => [
  ...ns.namespaces.map((name) => ({ title: name, value: name })),
  { divider: true },
  {
    title: "Create Namespace…",
    value: CREATE_SENTINEL,
    props: {
      prependIcon: "mdi-plus",
      class: "text-primary",
    },
  },
]);

// Intercept the sentinel before it lands in the store — v-select fires
// update:modelValue synchronously, so we swap the selection back to the
// previous namespace and open the dialog instead.
const previousNs = ref<string | null>(null);
const onNsSelected = (value: string) => {
  if (value === CREATE_SENTINEL) {
    // Revert selection so the label doesn't display the sentinel string
    // while the dialog is open.
    ns.selected = previousNs.value ?? ns.namespaces[0] ?? "";
    showCreateNs.value = true;
  } else {
    previousNs.value = value;
    ns.selected = value;
  }
};

const onNsCreated = (name: string) => {
  ns.fetchNamespaces();
  ns.selected = name;
  previousNs.value = name;
};
</script>

<template>
  <v-app-bar color="surface" density="comfortable" flat>
    <template v-slot:prepend>
      <div class="d-flex align-center pl-2">
        <div class="d-flex align-center" style="cursor: pointer" @click="router.push({ name: 'Applications' })">
          <v-icon
            icon="mdi-ship-wheel"
            size="36"
            color="primary"
            class="mr-3"
          />
          <span
            class="text-h5 font-weight-bold mr-4"
            style="letter-spacing: 0.5px; font-family: 'Roboto', sans-serif;"
          >
            Deckwatch
          </span>
        </div>
        <v-autocomplete
          :model-value="ns.selected"
          :items="nsItems"
          label="Namespace"
          role="combobox"
          aria-label="Select namespace"
          data-testid="namespace-switcher"
          density="compact"
          variant="outlined"
          hide-details
          auto-select-first
          style="min-width: 200px; max-width: 240px"
          :loading="ns.loading"
          @update:model-value="onNsSelected"
        >
          <template #item="{ props: itemProps, item }">
            <v-divider v-if="(item.raw as { divider?: boolean }).divider" />
            <v-list-item v-else v-bind="itemProps" />
          </template>
        </v-autocomplete>
      </div>
    </template>

    <template v-slot:append>
      <v-spacer />
      <v-btn variant="text" size="small" :to="{ name: 'Applications' }" class="mr-1">
        Applications
      </v-btn>
      <v-btn variant="text" size="small" :to="{ name: 'Deployments' }" class="mr-1">
        Resources
      </v-btn>
      <v-btn v-if="features?.registry" variant="text" size="small" :to="{ name: 'Registry' }" class="mr-1">
        Registry
      </v-btn>
      <v-btn
        variant="text"
        size="small"
        prepend-icon="mdi-server"
        class="mr-1"
        @click="router.push({ name: 'ClusterOverview' })"
      >
        Cluster
      </v-btn>
      <v-spacer />
      <v-btn
        :icon="isDark ? 'mdi-weather-sunny' : 'mdi-weather-night'"
        size="small"
        variant="text"
        class="mr-1"
        @click="toggleTheme"
      />
      <v-btn
        icon="mdi-cog"
        size="small"
        variant="text"
        class="mr-2"
        @click="router.push({ name: 'Settings' })"
      />
      <v-menu v-if="auth.isEnabled()" location="bottom end">
        <template #activator="{ props: activatorProps }">
          <v-btn
            v-bind="activatorProps"
            icon="mdi-account-circle"
            size="small"
            variant="text"
            class="mr-2"
          />
        </template>
        <v-list density="compact" min-width="220">
          <v-list-item v-if="auth.user.value">
            <v-list-item-title>{{ auth.user.value.name || "Signed in" }}</v-list-item-title>
            <v-list-item-subtitle>{{ auth.user.value.email }}</v-list-item-subtitle>
          </v-list-item>
          <v-divider v-if="auth.user.value" />
          <v-list-item prepend-icon="mdi-logout" title="Sign out" @click="auth.logout()" />
        </v-list>
      </v-menu>
    </template>
  </v-app-bar>

  <v-main>
    <v-container fluid>
      <v-alert v-if="ns.error" type="error" class="mb-4" closable>
        {{ ns.error }}
      </v-alert>
      <router-view />
    </v-container>
  </v-main>

  <v-footer class="text-caption text-secondary d-flex align-center justify-center ga-2" style="min-height: 20px; max-height: 20px; padding: 0 8px; font-size: 11px;">
    <span>Deckwatch v0.2.1</span>
    <span>·</span>
    <a href="/docs/book/" target="_blank" class="text-secondary" style="text-decoration: none;">Help</a>
    <span>·</span>
    <a href="/api/docs" target="_blank" class="text-secondary" style="text-decoration: none;">API</a>
  </v-footer>

  <CreateNamespaceDialog v-model="showCreateNs" @created="onNsCreated" />
</template>
