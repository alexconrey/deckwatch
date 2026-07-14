import { ref, computed } from "vue";
import { settingsApi } from "@/api/settings";
import type { DiagAgent } from "@/types/api";

// Server-backed AI provider toggles. These are loaded once from
// /api/settings on first use and cached module-wide so every consumer
// (DiagnoseButton, AiFixButton, SettingsPage) sees the same reactive state.

const _claudeEnabled = ref<boolean>(true);
const _codexEnabled = ref<boolean>(true);
let _loaded = false;
let _loadPromise: Promise<void> | null = null;

function loadFromServer(): Promise<void> {
  if (_loaded) return Promise.resolve();
  if (_loadPromise) return _loadPromise;
  _loadPromise = settingsApi
    .get()
    .then((s) => {
      _claudeEnabled.value = s.ai_claude_enabled ?? true;
      _codexEnabled.value = s.ai_codex_enabled ?? true;
      _loaded = true;
    })
    .catch(() => {
      // On failure, keep the defaults (both enabled). The next call
      // to loadFromServer() will retry.
      _loadPromise = null;
    });
  return _loadPromise;
}

// Kick off the fetch as soon as the module is imported so the values are
// ready by the time a component renders.
void loadFromServer();

// --- Per-browser preferred provider (the only thing that stays local) ---

const PREFERRED_KEY = "deckwatch-ai-preferred-provider";

function readPreferred(): DiagAgent | null {
  try {
    const raw = localStorage.getItem(PREFERRED_KEY);
    if (raw === "claude" || raw === "codex") return raw;
    return null;
  } catch {
    return null;
  }
}

const _preferredProvider = ref<DiagAgent | null>(readPreferred());

function persistPreferred() {
  try {
    if (_preferredProvider.value) {
      localStorage.setItem(PREFERRED_KEY, _preferredProvider.value);
    } else {
      localStorage.removeItem(PREFERRED_KEY);
    }
  } catch {
    // Storage unavailable; preference works in-memory for the session.
  }
}

export function useAiSettings() {
  // Read-only views of the server-side toggles. Components that need to
  // write (SettingsPage) manage their own refs and PUT back to the server;
  // these refs update on the next loadFromServer() call.
  const claudeEnabled = computed(() => _claudeEnabled.value);
  const codexEnabled = computed(() => _codexEnabled.value);

  const preferredProvider = computed<DiagAgent | null>({
    get: () => _preferredProvider.value,
    set: (v) => {
      _preferredProvider.value = v;
      persistPreferred();
    },
  });

  /** Force a re-fetch from the server (call after saving settings). */
  function refresh() {
    _loaded = false;
    _loadPromise = null;
    return loadFromServer();
  }

  return {
    claudeEnabled,
    codexEnabled,
    preferredProvider,
    refresh,
    /** @deprecated kept for backward compat; Codex is not selectable yet */
    codexComingSoon: true as const,
  };
}
