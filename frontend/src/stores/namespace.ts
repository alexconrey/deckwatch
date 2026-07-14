import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { namespacesApi } from "@/api/namespaces";

const STORAGE_KEY = "deckwatch-namespace";

export const useNamespaceStore = defineStore("namespace", () => {
  const namespaces = ref<string[]>([]);
  const selected = ref("");
  const loading = ref(false);
  const error = ref<string | null>(null);

  const fetchNamespaces = async () => {
    loading.value = true;
    error.value = null;
    try {
      const response = await namespacesApi.list();
      namespaces.value = response.namespaces;
      if (!selected.value && namespaces.value.length > 0) {
        const stored = localStorage.getItem(STORAGE_KEY);
        if (stored && namespaces.value.includes(stored)) {
          selected.value = stored;
        } else {
          selected.value = namespaces.value[0];
        }
      }
    } catch (e) {
      error.value =
        e instanceof Error ? e.message : "Failed to fetch namespaces";
    } finally {
      loading.value = false;
    }
  };

  watch(selected, (ns) => {
    if (ns) {
      localStorage.setItem(STORAGE_KEY, ns);
    }
  });

  return { namespaces, selected, loading, error, fetchNamespaces };
});
