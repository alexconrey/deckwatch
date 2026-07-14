import { apiFetch } from "./client";
import type { NodeListResponse } from "@/types/api";

export const nodesApi = {
  list: () => apiFetch<NodeListResponse>("/nodes"),
};
