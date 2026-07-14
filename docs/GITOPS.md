# Deckwatch GitOps

Deckwatch continuously polls Git repositories for new commits on a tracked
branch and, when it finds one, builds a container image with Kaniko and rolls
the new tag out to the associated Deployment. This document describes the
runtime model, the settings-driven managed lists that power the GitOps
configuration UI, the OCI registry support surface, and the live branch
listing API.

## Runtime model

Each GitOps-enabled Deployment stores its configuration as annotations under
the `deckwatch.io/` prefix. The watcher (see `src/watcher.rs`) polls every
10 seconds:

1. List Deployments per allowed namespace.
2. For every Deployment with `deckwatch.io/git-enabled=true`, fetch the
   remote HEAD SHA for its configured branch (via git's smart-HTTP
   `/info/refs?service=git-upload-pack` endpoint) using the token stored in
   the referenced K8s Secret.
3. If the SHA differs from the last-built one and the changed files clear
   the include/exclude path filters, create a Kaniko Job that clones the
   repository, builds the image, and pushes to the configured OCI registry.
4. When the Job completes, patch the Deployment's container image to
   `{oci_repository}:{short_sha}`, triggering a rolling update.

### Annotation schema

| Annotation                             | Purpose                                                            |
|----------------------------------------|--------------------------------------------------------------------|
| `deckwatch.io/git-enabled`             | `"true"` to enable GitOps for this deployment                      |
| `deckwatch.io/git-repo`                | HTTPS clone URL                                                    |
| `deckwatch.io/git-branch`              | Branch to track (default `main`)                                   |
| `deckwatch.io/git-token-secret`        | Name of the K8s Secret holding a `token` data key                  |
| `deckwatch.io/dockerfile-path`         | Path to Dockerfile inside the repo (default `Dockerfile`)          |
| `deckwatch.io/docker-context`          | Build context (default `.`)                                        |
| `deckwatch.io/oci-repository`          | **Canonical** OCI destination (e.g. `ghcr.io/org/api`)             |
| `deckwatch.io/ecr-repository`          | **Deprecated** — read as a fallback for legacy deployments only    |
| `deckwatch.io/include-paths`           | Comma-separated file/dir prefixes; if set, only these trigger      |
| `deckwatch.io/exclude-paths`           | Comma-separated prefixes to ignore                                 |
| `deckwatch.io/poll-interval`           | Reserved; polling is currently on a fixed 10s cadence              |
| `deckwatch.io/webhook-enabled`         | Reserved for future webhook support                                |
| `deckwatch.io/last-commit-sha`         | Last SHA the watcher observed                                      |
| `deckwatch.io/last-build-status`       | `building` / `success` / `failed`                                  |
| `deckwatch.io/last-build-job`          | Name of the most recent Kaniko Job                                 |
| `deckwatch.io/last-build-time`         | Timestamp of the most recent build attempt                         |
| `deckwatch.io/last-build-error`        | Error message from the most recent failure, if any                 |

## Settings-driven managed lists

Historically, the GitOps dialog required operators to free-type the
repository URL, K8s Secret name, and ECR repository for every deployment.
That is fragile: a typo silently produces a build failure minutes later.

The current model treats those values as **managed lists** in the
[Settings ConfigMap](SETTINGS.md), so the GitOps dialog offers dropdowns
populated from a small central inventory. The dialog also keeps a "Custom…"
option in each dropdown for one-off values that shouldn't pollute the shared
inventory.

### Schema additions

```jsonc
{
  "git_repositories": [
    {
      "name": "acme-api",
      "url": "https://github.com/acme/api",
      "default_branch": "main"
    }
  ],
  "oci_registries": [
    {
      "name": "acme-ecr",
      "url": "591839118651.dkr.ecr.us-gov-west-1.amazonaws.com/apps/api",
      "registry_type": "ecr"
    },
    {
      "name": "acme-ghcr",
      "url": "ghcr.io/acme/api",
      "registry_type": "ghcr"
    }
  ],
  "git_token_secrets": [
    {
      "name": "github-cicd",
      "secret_name": "github-cicd-token",
      "namespace": "deckwatch"
    }
  ]
}
```

- `name` is the display label shown in the dropdown and used as the
  selector key. It must be unique within each list.
- Git repository entries also carry `default_branch`, which pre-selects a
  branch when the repo is picked. The branch dropdown itself is still
  populated from a **live** query against the remote.
- OCI registry `registry_type` is one of `ecr`, `dockerhub`, `ghcr`, `gar`,
  `harbor`, `generic`. It is descriptive today (used for the UI icon and
  future auth-mode hints); the build path itself is registry-neutral.
- Git token entries reference a K8s Secret and its namespace. Because they
  are addressable by name, one Secret can be shared across many
  deployments — operators do not re-type the Secret name per deployment.

### Managing the lists

Settings → *Git Repositories* / *OCI Registries* / *Git Tokens* tabs
in the UI provide add/edit/remove for each list. The changes are persisted
into the `deckwatch-config` ConfigMap on Save. Alternatively, seed the
lists at install time in `helm/deckwatch/values.yaml` under
`settings.defaults`.

## OCI registry support

The build path is OCI-generic. Kaniko is invoked with:

```
--destination={oci_repository}:{short_sha}
```

That form works uniformly across:

- **Amazon ECR** — full registry URL, e.g. `591839118651.dkr.ecr.us-gov-west-1.amazonaws.com/apps/api`
- **Docker Hub** — `docker.io/myorg/api`
- **GitHub Container Registry** — `ghcr.io/myorg/api`
- **Google Artifact Registry** — `us-central1-docker.pkg.dev/proj/repo/api`
- **Harbor** and other self-hosted registries — `harbor.example.com/team/api`
- **Generic** OCI-compliant registries

Registry authentication is handled outside of the deckwatch config: the
Kaniko Job inherits whatever push credentials are available to the pod
(node IAM role for ECR/GAR, imagePullSecrets/service account for the
others). The `registry_type` field on `OciRegistry` is a hint for the UI
today and a foothold for automatic credential wiring in a future release.

### Backwards compatibility

Deployments created before the OCI-generic switch stored their target as
`deckwatch.io/ecr-repository`. The watcher and the API `GET` handler still
read that annotation as a fallback if `deckwatch.io/oci-repository` is
absent, so nothing needs to be re-configured after upgrade. On the next
edit, saving through the UI writes the canonical `oci-repository`
annotation. The `ecr_repository` field is still present in
`GitOpsConfig`/`GitOpsConfigRequest` JSON payloads as a mirror to keep
older frontend bundles rendering correctly during rolling upgrades.

## Live branch listing

The branch selector in the GitOps dialog is a `v-autocomplete` backed by a
new endpoint:

| Method | Path                | Purpose                                                            |
|--------|---------------------|--------------------------------------------------------------------|
| `GET`  | `/api/git/branches` | Enumerate branches on a remote repo, on demand                     |

### Query parameters

| Name           | Required | Meaning                                                                   |
|----------------|----------|---------------------------------------------------------------------------|
| `repo_url`     | yes      | HTTPS clone URL                                                           |
| `token_secret` | yes      | Managed token entry name (matches `git_token_secrets[].name`)             |
| `namespace`    | no       | Overrides the namespace of the token entry; needed only for shared tokens |

### Behavior

- The handler resolves `token_secret` against the settings ConfigMap to
  find the actual K8s Secret + namespace. If the caller passed a value not
  in the managed list, the endpoint returns `400` — this keeps callers
  from reaching arbitrary Secrets by guessing names.
- The Secret is read (must have a `token` data key), then the same git
  smart-HTTP endpoint that powers commit polling is used to enumerate
  refs. Refs matching `refs/heads/*` become branches.
- The response includes a `default_branch`, populated from the remote
  `HEAD` symref when present, otherwise `main`, then `master`, then the
  first branch, then `null`.
- Results are cached in-memory (per repo + token pair) for 30 seconds to
  avoid hammering the remote as the user opens/types in the dropdown.

### Example

```
GET /api/git/branches?repo_url=https%3A%2F%2Fgithub.com%2Forg%2Frepo&token_secret=github-cicd
```

```json
{
  "branches": ["develop", "feature/foo", "main", "release/v1"],
  "default_branch": "main"
}
```

## Onboarding checklist

1. Create the K8s Secret holding a `token` (`kubectl create secret generic
   github-cicd-token -n deckwatch --from-literal=token=ghp_...`). See
   [the TODO](/TODO.md) — an in-app Secret creation UI is on the roadmap.
2. Register the Secret in Settings → *Git Tokens*.
3. Add your repository in Settings → *Git Repositories*.
4. Add your registry in Settings → *OCI Registries*.
5. On the deployment detail page, open **GitOps → Enable**, pick the entries
   from the dropdowns, choose a branch from the live list, and save.
