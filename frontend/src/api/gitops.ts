import { apiFetch } from "./client";
import type {
  GitOpsStatus,
  GitOpsConfigRequest,
  BuildListResponse,
  BranchListResponse,
  JobPodListResponse,
} from "@/types/api";

export const gitopsApi = {
  getConfig: (ns: string, name: string) =>
    apiFetch<GitOpsStatus>(
      `/namespaces/${ns}/deployments/${name}/gitops`,
    ),

  setConfig: (ns: string, name: string, body: GitOpsConfigRequest) =>
    apiFetch<GitOpsStatus>(
      `/namespaces/${ns}/deployments/${name}/gitops`,
      { method: "PUT", body: JSON.stringify(body) },
    ),

  deleteConfig: (ns: string, name: string) =>
    apiFetch<void>(`/namespaces/${ns}/deployments/${name}/gitops`, {
      method: "DELETE",
    }),

  triggerBuild: (ns: string, name: string) =>
    apiFetch<{ message: string; job_name: string; commit_sha: string }>(
      `/namespaces/${ns}/deployments/${name}/gitops/trigger`,
      { method: "POST" },
    ),

  listBuilds: (ns: string, name: string) =>
    apiFetch<BuildListResponse>(
      `/namespaces/${ns}/deployments/${name}/gitops/builds`,
    ),

  listJobPods: (ns: string, jobName: string) =>
    apiFetch<JobPodListResponse>(
      `/namespaces/${ns}/jobs/${jobName}/pods`,
    ),

  getPodLogsHistory: (
    ns: string,
    podName: string,
    opts?: { container?: string; tailLines?: number },
  ) => {
    const params = new URLSearchParams();
    if (opts?.tailLines) params.set("tail_lines", String(opts.tailLines));
    if (opts?.container) params.set("container", opts.container);
    const qs = params.toString();
    return apiFetch<{ lines: string[] }>(
      `/namespaces/${ns}/pods/${podName}/logs/history${qs ? "?" + qs : ""}`,
    );
  },

  /**
   * Live branch listing for the GitOps dialog. `tokenSecret` is the name of
   * an entry in `settings.git_token_secrets` — the backend resolves it to a
   * K8s Secret. `namespace` is optional; when omitted the backend uses the
   * namespace defined on the token entry.
   *
   * Results are cached server-side for 30s to avoid hammering the remote.
   */
  listBranches: (opts: {
    repoUrl: string;
    tokenSecret: string;
    namespace?: string;
  }) => {
    const params = new URLSearchParams({
      repo_url: opts.repoUrl,
      token_secret: opts.tokenSecret,
    });
    if (opts.namespace) params.set("namespace", opts.namespace);
    return apiFetch<BranchListResponse>(`/git/branches?${params.toString()}`);
  },
};
