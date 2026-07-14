<script setup lang="ts">
import { computed } from "vue";
import { useLicense } from "@/composables/useLicense";

/**
 * Wraps a paywalled feature in a semi-transparent lock overlay with an
 * "Upgrade to Pro/Enterprise" CTA. Renders children untouched when the
 * current license grants the feature — no visual difference.
 *
 * Usage:
 * ```vue
 * <UpgradeOverlay feature="ai_diagnostics">
 *   <AiDiagnosticsPanel />
 * </UpgradeOverlay>
 * ```
 *
 * Per docs/LICENSING_STRATEGY.md §2.4: show paywalled features with an
 * upgrade affordance rather than hiding them. Discovery drives conversion.
 */

const props = defineProps<{
  feature: string;
  // Optional: allow the caller to override the CTA copy per feature.
  label?: string;
}>();

const { has, requiredTier, upgradeUrl } = useLicense();

const granted = computed(() => has(props.feature));
const tierRequired = computed(() => requiredTier(props.feature) ?? "pro");
const cta = computed(() => {
  if (props.label) return props.label;
  return tierRequired.value === "enterprise" ? "Contact Sales" : "Upgrade to Pro";
});
</script>

<template>
  <div v-if="granted">
    <slot />
  </div>
  <div v-else class="upgrade-overlay-wrapper">
    <div class="upgrade-overlay-content">
      <slot />
    </div>
    <div class="upgrade-overlay-scrim">
      <v-icon icon="mdi-lock" size="48" color="grey" class="mb-2" />
      <div class="text-h6 mb-1">
        {{ tierRequired === "enterprise" ? "Enterprise" : "Pro" }} feature
      </div>
      <div class="text-body-2 mb-3 text-medium-emphasis">
        This feature requires a
        {{ tierRequired === "enterprise" ? "Deckwatch Enterprise" : "Deckwatch Pro" }}
        license.
      </div>
      <v-btn
        :color="tierRequired === 'enterprise' ? 'success' : 'primary'"
        variant="flat"
        :href="upgradeUrl(feature)"
        target="_blank"
        rel="noopener"
      >
        {{ cta }}
      </v-btn>
    </div>
  </div>
</template>

<style scoped>
.upgrade-overlay-wrapper {
  position: relative;
}
.upgrade-overlay-content {
  filter: blur(3px);
  opacity: 0.4;
  pointer-events: none;
  user-select: none;
}
.upgrade-overlay-scrim {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  background: rgba(255, 255, 255, 0.55);
  text-align: center;
  padding: 1rem;
}
</style>
