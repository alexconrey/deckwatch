<script setup lang="ts">
import { computed } from "vue";
import { useClusterAlerts } from "@/composables/useClusterAlerts";

// Ambient stack of cluster-warning toasts, mounted once at the app root.
// v-snackbar is single-instance by design (calling it repeatedly clobbers
// the previous message), so we render our own stack of card elements
// pinned to the top-right of the viewport and drive their lifecycle from
// `useClusterAlerts`.

const { toasts, dismissToast, enabled } = useClusterAlerts();

// Hide the stack entirely when the user has disabled cluster alerts in
// Settings. The composable still ref-counts and keeps state consistent --
// it just does not surface anything.
const visibleToasts = computed(() => (enabled.value ? toasts.value : []));
</script>

<template>
  <Teleport to="body">
    <div class="alert-toast-stack" role="region" aria-live="polite">
      <TransitionGroup name="alert-toast">
        <div
          v-for="toast in visibleToasts"
          :key="toast.id"
          class="alert-toast"
          role="alert"
          tabindex="0"
          @click="dismissToast(toast.id)"
          @keydown.enter="dismissToast(toast.id)"
          @keydown.space.prevent="dismissToast(toast.id)"
        >
          <div class="alert-toast-icon">
            <v-icon icon="mdi-alert" color="warning" size="20" />
          </div>
          <div class="alert-toast-body">
            <div class="alert-toast-title">
              <span class="alert-toast-reason">{{ toast.reason }}</span>
              <span class="alert-toast-target">{{ toast.involvedLabel }}</span>
            </div>
            <div v-if="toast.message" class="alert-toast-message">
              {{ toast.message }}
            </div>
          </div>
          <v-btn
            icon="mdi-close"
            size="x-small"
            variant="text"
            density="compact"
            class="alert-toast-close"
            aria-label="Dismiss alert"
            @click.stop="dismissToast(toast.id)"
          />
        </div>
      </TransitionGroup>
    </div>
  </Teleport>
</template>

<style scoped>
.alert-toast-stack {
  position: fixed;
  top: 72px;
  right: 16px;
  z-index: 2400;
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-width: 380px;
  width: min(380px, calc(100vw - 32px));
  pointer-events: none;
}

.alert-toast {
  pointer-events: auto;
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px 12px;
  background-color: rgb(var(--v-theme-surface));
  color: rgb(var(--v-theme-on-surface));
  border-left: 4px solid rgb(var(--v-theme-warning));
  border-radius: 4px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.25);
  cursor: pointer;
  outline: none;
}

.alert-toast:focus-visible {
  box-shadow: 0 0 0 2px rgb(var(--v-theme-warning));
}

.alert-toast-icon {
  flex: 0 0 auto;
  padding-top: 2px;
}

.alert-toast-body {
  flex: 1 1 auto;
  min-width: 0;
}

.alert-toast-title {
  display: flex;
  align-items: baseline;
  flex-wrap: wrap;
  gap: 6px;
  font-size: 0.875rem;
  line-height: 1.25;
}

.alert-toast-reason {
  font-weight: 600;
  color: rgb(var(--v-theme-warning));
}

.alert-toast-target {
  font-size: 0.75rem;
  color: rgba(var(--v-theme-on-surface), 0.7);
  word-break: break-word;
}

.alert-toast-message {
  margin-top: 4px;
  font-size: 0.8125rem;
  line-height: 1.35;
  color: rgba(var(--v-theme-on-surface), 0.85);
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  word-break: break-word;
}

.alert-toast-close {
  flex: 0 0 auto;
  margin-left: 4px;
}

.alert-toast-enter-active,
.alert-toast-leave-active {
  transition: transform 200ms ease, opacity 200ms ease;
}

.alert-toast-enter-from {
  opacity: 0;
  transform: translateX(24px);
}

.alert-toast-leave-to {
  opacity: 0;
  transform: translateX(24px);
}

.alert-toast-move {
  transition: transform 200ms ease;
}
</style>
