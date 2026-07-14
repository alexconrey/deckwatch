import { defineStore } from "pinia";
import { ref } from "vue";
import { namespacesApi } from "@/api/namespaces";

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
        selected.value = namespaces.value[0];
      }
    } catch (e) {
      error.value =
        e instanceof Error ? e.message : "Failed to fetch namespaces";
    } finally {
      loading.value = false;
    }
  };

  return { namespaces, selected, loading, error, fetchNamespaces };
});
