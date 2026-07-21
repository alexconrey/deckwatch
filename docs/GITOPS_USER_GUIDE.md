# GitOps User Guide

GitOps in Deckwatch automates the build-and-deploy cycle: you push code to a
Git repository, and Deckwatch automatically builds a new container image and
deploys it. No manual image tagging, no separate CI pipeline to configure --
push a commit and the new version rolls out.

This guide covers setting up GitOps from a developer's perspective. For
operator-level configuration (managed repository lists, registry setup), see
the [GitOps](GITOPS.md) reference.

## What GitOps Does

When you enable GitOps on a deployment, here is what happens behind the
scenes:

1. **Deckwatch polls your Git repository** every 10 seconds, checking the
   HEAD commit on your configured branch.
2. **When it detects a new commit**, it creates a Kaniko build Job inside
   the cluster. Kaniko clones your repository, builds a container image from
   your Dockerfile, and pushes it to the configured container registry.
3. **When the build succeeds**, Deckwatch updates the deployment's container
   image to the newly built tag (the short commit SHA). Kubernetes then
   performs a rolling update to deploy the new version.
4. **Build results are recorded** in the build history, so you can see what
   was built, when, and whether it succeeded or failed.

The entire cycle -- from `git push` to running the new code -- typically takes
1-3 minutes, depending on build time.

## Prerequisites

Before setting up GitOps on a deployment, you need:

1. **A Git repository** with a Dockerfile.
2. **A Git access token** stored as a Kubernetes Secret. The token must have
   read access to the repository.
3. **A container registry** where built images will be pushed (for example,
   Amazon ECR, GitHub Container Registry, or Docker Hub).

Your cluster administrator may have already configured managed lists of
repositories, registries, and tokens in Deckwatch's Settings page. If so,
you will be able to select them from dropdowns rather than typing them
manually.

## Setting Up GitOps on a Deployment

### Step 1: Create a Git Token Secret (if needed)

If your repository is private, you need a Kubernetes Secret containing an
access token. Check with your administrator first -- they may have already
created one.

To create one yourself:

1. Generate a personal access token from your Git provider (GitHub, GitLab,
   etc.) with read access to the repository.
2. Navigate to **Resources** in the app bar.
3. Click the **Secrets** tab.
4. Click **Create Secret**.
5. Enter a **Name** (for example, `github-token`).
6. Add a key-value pair with **Key** = `token` and **Value** = your access
   token.
7. Click **Create**.

**Important:** The Secret must have a key named `token`. This is the key
Deckwatch looks for when authenticating to the Git repository.

If your administrator has already registered the Secret in Settings under
**Git Tokens**, you can select it by name when configuring GitOps instead
of referencing the raw Secret name.

### Step 2: Configure GitOps on the Deployment

1. Navigate to the deployment's detail page.
2. Find the **GitOps** card.
3. Click **Enable** (or **Configure** if it is already enabled).
4. Fill in the configuration:

| Field | What to Enter |
|-------|---------------|
| **Repository** | The HTTPS clone URL of your Git repository. If your administrator configured managed repositories, select one from the dropdown. Otherwise, type a custom URL (for example, `https://github.com/myorg/myrepo`). |
| **Branch** | The branch to track. Select from the dropdown (Deckwatch queries the remote for available branches) or type a custom branch name. Defaults to `main`. |
| **Token Secret** | The name of the Kubernetes Secret holding your Git access token. Select from the managed list or enter a custom Secret name. |
| **Dockerfile Path** | The path to the Dockerfile inside the repository. Defaults to `Dockerfile`. Change this if your Dockerfile is in a subdirectory (for example, `docker/Dockerfile.prod`). |
| **Build Context** | The Docker build context (the directory sent to the builder). Defaults to `.` (the repository root). |
| **Registry** | Where to push the built image. Select from the managed list or enter a custom registry URL (for example, `ghcr.io/myorg/myapp`). |

5. Click **Save**.

GitOps is now active. Deckwatch will start polling the repository immediately.

### Step 3: Configure Path Filters (optional)

If your repository contains code for multiple services, you can configure
path filters so that only changes to specific files trigger a build.

| Filter | Purpose |
|--------|---------|
| **Include Paths** | Only trigger a build when files in these paths change. For example, `src/,Dockerfile` means only changes to files under `src/` or the Dockerfile will trigger a build. |
| **Exclude Paths** | Never trigger a build for changes in these paths. For example, `docs/,README.md` means documentation changes will not trigger a build. |

If both include and exclude paths are set, include paths are checked first.
A file must match an include path AND not match any exclude path to trigger
a build.

Leave both blank to trigger a build on every commit (the default).

## Monitoring Builds

After GitOps is configured, the **GitOps** card on the deployment detail page
shows:

- **Status** -- the current GitOps state:
  - `success` (green) -- the last build completed successfully.
  - `building` (blue) -- a build is in progress.
  - `failed` (red) -- the last build failed.
  - `pending` -- waiting for the first commit to be detected.
- **Last Commit** -- the SHA of the most recent commit Deckwatch observed.
- **Last Build Time** -- when the most recent build was started.
- **Last Build Error** -- if the build failed, the error message.

### Build History

The GitOps card includes a build history table showing recent builds:

| Column | What It Shows |
|--------|---------------|
| **Commit** | The short SHA of the commit that triggered the build. |
| **Image Tag** | The tag assigned to the built image (matches the commit SHA). |
| **Status** | `success`, `failed`, or `building`. |
| **Started** | When the build started. |
| **Completed** | When the build finished (blank if still running). |

## Triggering a Manual Build

You can trigger a build manually without pushing a new commit:

1. In the **GitOps** card, click **Trigger Build**.
2. Deckwatch checks the current HEAD of the configured branch and starts a
   build, even if the commit has not changed.

This is useful when:
- A previous build failed due to a transient issue (registry timeout, node
  resource pressure) and you want to retry.
- You changed something outside the repository (for example, a base image
  was updated) and want to rebuild.

## What Happens When You Push a Commit

Here is the full sequence after a `git push`:

1. Within 10 seconds, Deckwatch's poller detects the new commit SHA on the
   tracked branch.
2. If path filters are configured, Deckwatch checks which files changed. If
   all changed files are excluded, no build is triggered.
3. Deckwatch creates a Kaniko Job in the deployment's namespace. The Job:
   - Clones the repository using the configured token.
   - Runs `docker build` using Kaniko (no Docker daemon needed).
   - Pushes the built image to the configured registry with a tag matching
     the short commit SHA.
4. The GitOps card updates to show `building` status.
5. When the build succeeds:
   - Deckwatch patches the deployment's container image to the new tag.
   - Kubernetes performs a rolling update.
   - The GitOps card shows `success`.
6. When the build fails:
   - The GitOps card shows `failed` with an error message.
   - The deployment continues running the previous image.
   - Check the build logs (click the failed build in the history) for
     details.

### Webhook Triggers

In addition to polling, Deckwatch can receive Git push webhooks from GitHub
and GitLab. When a webhook arrives, the build starts immediately instead of
waiting for the next 10-second poll cycle.

If your administrator has configured webhook support, push events trigger
near-instant builds. The webhook URL is:

```
POST https://your-deckwatch-url/api/webhooks/git
```

## Disabling GitOps

To stop automatic builds and deploys:

1. Navigate to the deployment's detail page.
2. In the **GitOps** card, click **Disable**.

The deployment keeps running its current image. No further builds will be
triggered until you re-enable GitOps.

## Troubleshooting

### Build fails with "authentication required"

The Git token Secret is missing, has the wrong key name, or the token has
expired.

1. Check that the Secret exists in the same namespace as the deployment.
2. Verify the Secret has a key named `token` (not `password`, `pat`, etc.).
3. Generate a new token from your Git provider and update the Secret.

### Build fails with "Dockerfile not found"

The `Dockerfile Path` setting does not match the actual location in the
repository.

1. Check the path in the GitOps configuration. The default is `Dockerfile`
   in the repository root.
2. If your Dockerfile is elsewhere, update the path (for example,
   `build/Dockerfile`).

### Build succeeds but the deployment does not update

1. Check the deployment detail page -- the image tag should show the new
   commit SHA.
2. If the image tag updated but pods are still running the old version, check
   the pod events for image pull errors.
3. If the image tag did not update, check whether the build actually pushed
   to the registry URL configured for the deployment.

### No builds are triggered even though commits are being pushed

1. Check path filters. If include paths are set, only changes to those paths
   trigger builds. Try removing the filters temporarily to confirm.
2. Verify the branch name matches what you are pushing to.
3. Check that the Deckwatch pod is running and can reach your Git hosting
   provider (network/firewall issues).
