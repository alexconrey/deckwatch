import { computed, readonly, ref } from "vue";
import { licenseApi, type FeatureCatalogEntry, type LicenseStatus, type Tier } from "@/api/license";

/**
 * License / entitlements composable.
 *
 * Backend endpoint `GET /api/license` returns the current tier, expiry, and
 * granted features. This composable caches the response module-scope so every
 * component that needs to gate UI reads the same object, and exposes a
 * `has(feature)` helper for the standard "should I render this?" check.
 *
 * ## Community pledge
 *
 * Cluster-control features are never gated on the frontend either — do NOT
 * call `has(...)` for anything under Deployments, Pods, ConfigMaps, Secrets,
 * exec, port-forward, logs, or GitOps. Those must render for every user
 * regardless of `licenseState.tier`. See `docs/LICENSING_STRATEGY.md` §1.
 *
 * ## Upgrade overlay pattern
 *
 * For Pro/Enterprise features, prefer rendering the feature UI with a
 * disabled overlay that promotes the upgrade, rather than hiding the feature
 * entirely. Discovery drives conversion — hidden features drive zero
 * conversion. See §2.4 of the strategy doc.
 */

const licenseState = ref<LicenseStatus | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const lastLoadedAt = ref<number | null>(null);

// Refresh no more than once per minute unless forced. Licenses don't change
// often at runtime; pounding the endpoint on every mount is wasteful.
const CACHE_TTL_MS = 60_000;

async function load(force = false): Promise<void> {
  if (
    !force &&
    licenseState.value &&
    lastLoadedAt.value &&
    Date.now() - lastLoadedAt.value < CACHE_TTL_MS
  ) {
    return;
  }
  loading.value = true;
  error.value = null;
  try {
    licenseState.value = await licenseApi.get();
    lastLoadedAt.value = Date.now();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
    // On failure, degrade to a synthetic Community response so callers of
    // has() get a defined answer. This mirrors backend behavior: Community
    // is the safe fallback everywhere.
    licenseState.value = {
      tier: "community",
      expires_at: null,
      in_grace: false,
      features: [],
      feature_catalog: [],
      limits: { max_users: null, max_clusters: null },
      customer: null,
      license_id: null,
      seconds_until_expiry: null,
    };
  } finally {
    loading.value = false;
  }
}

const tier = computed<Tier>(() => licenseState.value?.tier ?? "community");
const inGrace = computed<boolean>(() => licenseState.value?.in_grace ?? false);
const expiresAt = computed<Date | null>(() => {
  const raw = licenseState.value?.expires_at;
  return raw ? new Date(raw) : null;
});
const daysUntilExpiry = computed<number | null>(() => {
  const secs = licenseState.value?.seconds_until_expiry;
  if (secs == null) return null;
  return Math.floor(secs / 86400);
});

/**
 * Is `feature` granted by the current license? Returns false for Community
 * fallback and for any feature not in the granted set. Safe to call before
 * the license has loaded — returns false until data arrives.
 */
function has(feature: string): boolean {
  return licenseState.value?.features.includes(feature) ?? false;
}

/**
 * Minimum tier required to unlock `feature`. Looks it up in the backend's
 * feature catalog so the frontend doesn't hard-code the mapping. Returns
 * `null` for an unknown feature (which is a bug — features must exist in the
 * catalog to be gate-able).
 */
function requiredTier(feature: string): Tier | null {
  const entry = licenseState.value?.feature_catalog.find((e) => e.feature === feature);
  return entry?.tier ?? null;
}

function featureCatalog(): FeatureCatalogEntry[] {
  return licenseState.value?.feature_catalog ?? [];
}

/**
 * Build a deep-link to the pricing page for a specific feature. Mirrors the
 * `upgrade_url` the backend returns in 403 responses so the SPA and API
 * agree on the destination.
 */
function upgradeUrl(feature: string): string {
  const t = requiredTier(feature) ?? "pro";
  return `https://deckwatch.io/pricing?feature=${encodeURIComponent(feature)}&tier=${t}`;
}

/** Force-refresh — call after license file changes (support ticket flow). */
async function refresh(): Promise<void> {
  await load(true);
}

export function useLicense() {
  return {
    // reactive state
    state: readonly(licenseState),
    tier,
    inGrace,
    expiresAt,
    daysUntilExpiry,
    loading: readonly(loading),
    error: readonly(error),
    // actions
    load,
    refresh,
    // predicates
    has,
    requiredTier,
    featureCatalog,
    upgradeUrl,
  };
}
