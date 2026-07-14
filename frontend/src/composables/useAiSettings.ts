import { computed, ref } from "vue";

// Persisted-in-localStorage toggles for which AI providers are enabled.
// Kept as module-scoped refs so every caller sees the same reactive state
// without extra plumbing (Pinia store, provide/inject, etc.).

const STORAGE_KEY = "deckwatch-ai-settings";

type PersistedShape = {
  claude?: boolean;
  codex?: boolean;
};

function readInitial(): PersistedShape {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as PersistedShape;
    return parsed ?? {};
  } catch {
    return {};
  }
}

const initial = readInitial();

// Claude defaults ON (the shipping provider). Codex is not selectable yet.
const _claudeEnabled = ref<boolean>(initial.claude ?? true);
const _codexEnabled = ref<boolean>(false);

function persist() {
  const payload: PersistedShape = {
    claude: _claudeEnabled.value,
    codex: _codexEnabled.value,
  };
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
  } catch {
    // Storage may be unavailable (private mode, disabled quota). Ignore —
    // toggles will still work in-memory for the session.
  }
}

export function useAiSettings() {
  const claudeEnabled = computed<boolean>({
    get: () => _claudeEnabled.value,
    set: (v) => {
      _claudeEnabled.value = v;
      persist();
    },
  });

  // Codex remains hard-locked to false until the backend supports it. Writes
  // are ignored so the toggle in Settings can bind to it without accidentally
  // flipping it on.
  const codexEnabled = computed<boolean>({
    get: () => _codexEnabled.value,
    set: () => {},
  });

  return {
    claudeEnabled,
    codexEnabled,
    codexComingSoon: true as const,
  };
}
