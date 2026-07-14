import { defineStore } from "pinia";
import { ref } from "vue";
import { applicationsApi } from "@/api/applications";
import type { ApplicationSummary } from "@/types/api";

export const useApplicationsStore = defineStore("applications", () => {
  const applications = ref<ApplicationSummary[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);

  const fetchApplications = async (namespace: string) => {
    if (!namespace) return;
    loading.value = true;
    error.value = null;
    try {
      const response = await applicationsApi.list(namespace);
      applications.value = response.applications;
    } catch (e) {
      error.value =
        e instanceof Error ? e.message : "Failed to fetch applications";
    } finally {
      loading.value = false;
    }
  };

  return { applications, loading, error, fetchApplications };
});
