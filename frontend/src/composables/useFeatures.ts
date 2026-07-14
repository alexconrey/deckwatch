import { ref, onMounted } from "vue";
import { apiFetch } from "@/api/client";

interface Features {
  prometheus: boolean;
  registry: boolean;
}

const features = ref<Features | null>(null);

async function load() {
  try {
    features.value = await apiFetch<Features>("/features");
  } catch {
    features.value = { prometheus: false, registry: false };
  }
}

export function useFeatures() {
  onMounted(load);
  return { features, refresh: load };
}
