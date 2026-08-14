import { apiFetch } from "./client";
import type {
  AgentFeedbackListResponse,
  AgentFeedbackItem,
  AgentFeedbackStatus,
} from "@/types/api";

export const agentFeedbackApi = {
  list: (opts?: { status?: AgentFeedbackStatus; category?: string }) => {
    const params = new URLSearchParams();
    if (opts?.status) params.set("status", opts.status);
    if (opts?.category) params.set("category", opts.category);
    const qs = params.toString();
    return apiFetch<AgentFeedbackListResponse>(`/agent-feedback${qs ? "?" + qs : ""}`);
  },

  updateStatus: (id: string, status: AgentFeedbackStatus) =>
    apiFetch<AgentFeedbackItem>(`/agent-feedback/${id}`, {
      method: "PATCH",
      body: JSON.stringify({ status }),
    }),
};
