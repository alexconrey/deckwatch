<script setup lang="ts">
import { computed, onMounted } from "vue";
import { useLicense } from "@/composables/useLicense";
import type { Tier } from "@/api/license";

/**
 * Settings > License panel.
 *
 * Renders:
 *  - Current tier chip (Community / Pro / Enterprise) with expiry + grace banner
 *  - Customer / license-ID info block for support tickets
 *  - Full feature matrix — every gate-able feature the backend advertises,
 *    marked granted or not, with a per-row upgrade link for locked features
 *
 * This panel is safe to render at any tier; there is no license required to
 * view license state (per docs/LICENSING_STRATEGY.md §2.4).
 */

const { state, tier, inGrace, expiresAt, daysUntilExpiry, loading, error, load, has, upgradeUrl } =
  useLicense();

onMounted(() => {
  load();
});

const tierColor = computed(() => {
  const map: Record<Tier, string> = {
    community: "grey",
    pro: "primary",
    enterprise: "success",
  };
  return map[tier.value];
});

const tierLabel = computed(() => tier.value.charAt(0).toUpperCase() + tier.value.slice(1));

const expiryText = computed(() => {
  const d = expiresAt.value;
  if (!d) return "No expiry";
  return d.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
});

const graceBanner = computed(() => {
  if (!inGrace.value) return null;
  const days = daysUntilExpiry.value ?? 0;
  // In-grace means seconds_until_expiry is negative — days is negative too.
  const daysPast = Math.abs(days);
  return {
    color: "warning",
    text: `Your license expired ${daysPast} day${daysPast === 1 ? "" : "s"} ago. ` +
          `Pro features remain active during a 30-day grace period. Renew to avoid disruption.`,
  };
});

const expiredBanner = computed(() => {
  // Past grace: tier will have been downgraded to community, but customer info
  // is still present. Show the "please renew" message.
  if (tier.value !== "community" || !state.value?.customer) return null;
  return {
    color: "error",
    text: `Your license expired past the grace period. Deckwatch is running in Community mode. Contact ${state.value.customer.contact} to renew.`,
  };
});

// Group catalog by tier so the matrix is easier to scan.
const proFeatures = computed(() =>
  (state.value?.feature_catalog ?? []).filter((f) => f.tier === "pro"),
);
const enterpriseFeatures = computed(() =>
  (state.value?.feature_catalog ?? []).filter((f) => f.tier === "enterprise"),
);
</script>

<template>
  <v-card variant="flat" class="pa-4">
    <div class="d-flex align-center mb-4">
      <h2 class="text-h5 mr-3">License</h2>
      <v-chip :color="tierColor" size="small" variant="flat">
        {{ tierLabel }}
      </v-chip>
      <v-spacer />
      <v-btn size="small" variant="text" :loading="loading" @click="load(true)">
        Refresh
      </v-btn>
    </div>

    <v-alert v-if="error" type="error" variant="tonal" density="compact" class="mb-3">
      Failed to load license: {{ error }}
    </v-alert>

    <v-alert
      v-if="graceBanner"
      :type="graceBanner.color as any"
      variant="tonal"
      density="compact"
      class="mb-3"
    >
      {{ graceBanner.text }}
    </v-alert>

    <v-alert
      v-if="expiredBanner"
      :type="expiredBanner.color as any"
      variant="tonal"
      density="compact"
      class="mb-3"
    >
      {{ expiredBanner.text }}
    </v-alert>

    <v-row v-if="state" class="mb-4">
      <v-col cols="12" md="6">
        <div class="text-caption text-medium-emphasis">Expires</div>
        <div>{{ expiryText }}</div>
      </v-col>
      <v-col cols="12" md="6">
        <div class="text-caption text-medium-emphasis">Customer</div>
        <div v-if="state.customer">
          {{ state.customer.org }} &lt;{{ state.customer.contact }}&gt;
        </div>
        <div v-else class="text-medium-emphasis">— (Community, no customer record)</div>
      </v-col>
      <v-col v-if="state.limits.max_users != null" cols="12" md="6">
        <div class="text-caption text-medium-emphasis">User limit</div>
        <div>{{ state.limits.max_users }}</div>
      </v-col>
      <v-col v-if="state.limits.max_clusters != null" cols="12" md="6">
        <div class="text-caption text-medium-emphasis">Cluster limit</div>
        <div>{{ state.limits.max_clusters }}</div>
      </v-col>
      <v-col v-if="state.license_id" cols="12">
        <div class="text-caption text-medium-emphasis">License ID</div>
        <code class="text-body-2">{{ state.license_id }}</code>
      </v-col>
    </v-row>

    <v-divider class="my-4" />

    <h3 class="text-subtitle-1 mb-2">Pro features</h3>
    <v-list density="compact" class="mb-4">
      <v-list-item v-for="f in proFeatures" :key="f.feature">
        <template #prepend>
          <v-icon
            :icon="has(f.feature) ? 'mdi-check-circle' : 'mdi-lock-outline'"
            :color="has(f.feature) ? 'success' : 'grey'"
          />
        </template>
        <v-list-item-title>{{ f.label }}</v-list-item-title>
        <v-list-item-subtitle>{{ f.description }}</v-list-item-subtitle>
        <template #append>
          <v-btn
            v-if="!has(f.feature)"
            size="x-small"
            variant="outlined"
            color="primary"
            :href="upgradeUrl(f.feature)"
            target="_blank"
            rel="noopener"
          >
            Upgrade to Pro
          </v-btn>
        </template>
      </v-list-item>
    </v-list>

    <h3 class="text-subtitle-1 mb-2">Enterprise features</h3>
    <v-list density="compact">
      <v-list-item v-for="f in enterpriseFeatures" :key="f.feature">
        <template #prepend>
          <v-icon
            :icon="has(f.feature) ? 'mdi-check-circle' : 'mdi-lock-outline'"
            :color="has(f.feature) ? 'success' : 'grey'"
          />
        </template>
        <v-list-item-title>{{ f.label }}</v-list-item-title>
        <v-list-item-subtitle>{{ f.description }}</v-list-item-subtitle>
        <template #append>
          <v-btn
            v-if="!has(f.feature)"
            size="x-small"
            variant="outlined"
            color="success"
            :href="upgradeUrl(f.feature)"
            target="_blank"
            rel="noopener"
          >
            Contact Sales
          </v-btn>
        </template>
      </v-list-item>
    </v-list>
  </v-card>
</template>
