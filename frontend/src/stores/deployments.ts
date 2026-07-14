import { defineStore } from "pinia";
import { ref } from "vue";
import { deploymentsApi } from "@/api/deployments";
import type { DeploymentSummary } from "@/types/api";

export const useDeploymentsStore = defineStore("deployments", () => {
  const deployments = ref<DeploymentSummary[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);

  const fetchDeployments = async (namespace: string) => {
    if (!namespace) return;
    loading.value = true;
    error.value = null;
    try {
      const response = await deploymentsApi.list(namespace);
      deployments.value = response.deployments;
    } catch (e) {
      error.value =
        e instanceof Error ? e.message : "Failed to fetch deployments";
    } finally {
      loading.value = false;
    }
  };

  return { deployments, loading, error, fetchDeployments };
});
