import { computed, ref } from "vue";

// Persisted-in-localStorage toggle for whether cluster warning-event toast
// notifications appear in the top-right stack. Kept as a module-scoped ref so
// every caller sees the same reactive state without extra plumbing.

const STORAGE_KEY = "deckwatch-cluster-alerts";

type PersistedShape = {
  enabled?: boolean;
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

// Defaults ON -- operators generally want to know when their cluster is
// unhappy without having to opt in. They can silence via Settings.
const _enabled = ref<boolean>(initial.enabled ?? true);

function persist() {
  const payload: PersistedShape = { enabled: _enabled.value };
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
  } catch {
    // Storage may be unavailable (private mode, disabled quota). Ignore --
    // toggle will still work in-memory for the session.
  }
}

export function useClusterAlertSettings() {
  const enabled = computed<boolean>({
    get: () => _enabled.value,
    set: (v) => {
      _enabled.value = v;
      persist();
    },
  });

  return { enabled };
}
