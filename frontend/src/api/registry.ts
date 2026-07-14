import { apiFetch } from "./client";

export interface RegistryEnabledResponse {
  enabled: boolean;
}

export interface RepositorySummary {
  name: string;
  tag_count: number;
  total_size: number;
}

export interface RepositoryListResponse {
  repositories: RepositorySummary[];
}

export interface TagSummary {
  tag: string;
  digest: string;
  media_type: string;
  size: number;
  created: string | null;
}

export interface TagListPayload {
  name: string;
  tags: TagSummary[];
}

export interface LayerSummary {
  digest: string;
  media_type: string;
  size: number;
}

export interface ManifestDetail {
  name: string;
  tag: string;
  digest: string;
  media_type: string;
  config: LayerSummary | null;
  layers: LayerSummary[];
  total_size: number;
  // Raw manifest JSON as pushed; typed as unknown because the shape varies
  // by media type (OCI image manifest, Docker manifest v2, index, etc).
  manifest: unknown;
}

// The name path segment may contain `/` for nested repos (`myorg/api`).
// encodeURIComponent would escape those slashes, which the backend needs
// to see literally, so we pass the raw name and only encode reserved chars.
const encodeRepo = (name: string): string =>
  name
    .split("/")
    .map((s) => encodeURIComponent(s))
    .join("/");

export const registryApi = {
  enabled: () => apiFetch<RegistryEnabledResponse>("/registry/enabled"),

  listRepositories: () =>
    apiFetch<RepositoryListResponse>("/registry/repositories"),

  listTags: (name: string) =>
    apiFetch<TagListPayload>(
      `/registry/repositories/${encodeRepo(name)}/tags`,
    ),

  getManifest: (name: string, tag: string) =>
    apiFetch<ManifestDetail>(
      `/registry/repositories/${encodeRepo(name)}/tags/${encodeURIComponent(tag)}`,
    ),

  deleteTag: (name: string, tag: string) =>
    apiFetch<void>(
      `/registry/repositories/${encodeRepo(name)}/tags/${encodeURIComponent(tag)}`,
      { method: "DELETE" },
    ),
};
