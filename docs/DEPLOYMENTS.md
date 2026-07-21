# Deployments

A deployment is the core building block in Deckwatch. It tells Kubernetes to
run one or more copies (replicas) of a container image and keep them running.
This guide covers everything you can do with deployments in the Deckwatch UI.

## Creating a Deployment

1. Select a namespace from the app bar.
2. Navigate to **Resources** in the app bar.
3. Click **Create Deployment**.
4. Fill in the form (see below for field details).
5. Click **Create**.

You will be redirected to the new deployment's detail page.

### Form Fields

| Field | Required | Description |
|-------|----------|-------------|
| **Name** | Yes | A unique name within the namespace. Use lowercase letters, numbers, and hyphens. Must start and end with a letter or number. Maximum 253 characters. |
| **Image** | Yes | The container image to run, such as `nginx:latest` or `ghcr.io/myorg/api:v1.2.3`. You can also click the tag picker icon to browse images from the embedded registry. |
| **Replicas** | No | How many copies of the container to run. Defaults to 1. |
| **Ports** | No | Container ports to expose. Each entry has a port number, an optional name, and a protocol (TCP by default). |
| **Environment Variables** | No | Key-value pairs injected into the container as environment variables. Click **Add** to add rows. |
| **Command** | No | Overrides the container's default entrypoint. Separate arguments with spaces. |
| **Args** | No | Arguments passed to the command. Separate with spaces. |
| **CPU Request** | No | The minimum CPU the container needs. See "Resource Units" below. |
| **Memory Request** | No | The minimum memory the container needs. |
| **CPU Limit** | No | The maximum CPU the container can use. |
| **Memory Limit** | No | The maximum memory the container can use. If exceeded, the container is killed. |
| **Liveness Probe** | No | Checks whether the container is still alive. See "Health Probes" below. |
| **Readiness Probe** | No | Checks whether the container is ready to receive traffic. |
| **Startup Probe** | No | Gives slow-starting containers time to initialize before liveness checks begin. |

### Using Templates

Instead of filling in every field from scratch, you can start from a template.
Click the **Templates** button at the top of the Create Deployment page. The
template picker shows pre-configured defaults for common application types:

- **Web App** -- a web server with a readiness probe on port 80.
- **Worker** -- a background process with no ports exposed.
- **Static Site** -- an nginx container serving static files.

Select a template and it will pre-fill the form. You can then adjust any
field before creating.

### Dry-Run Validation

Before you create, you can click **Validate** to check for issues. Deckwatch
sends a dry-run request to the Kubernetes API and shows any errors inline --
for example, if the image name is malformed or resource values are invalid.

## Editing a Deployment

1. Navigate to the deployment's detail page (click its name in the Resources
   list).
2. Click the **Edit** button in the upper right.
3. An edit dialog opens with two modes:
   - **Form** -- the same form used during creation, pre-filled with current
     values. You can change any field except the name.
   - **YAML** -- a raw YAML editor for the full Kubernetes Deployment spec.
     Use this for advanced changes that the form does not cover.
4. Make your changes and click **Save** (for the form) or **Apply** (for
   YAML).

Changes take effect immediately. Kubernetes performs a rolling update --
replacing old pods with new ones gradually, so there is no downtime.

## Scaling

Scaling changes how many replica pods are running.

**From the deployment detail page:**

1. Click **Edit**.
2. Adjust the **Replicas** slider or enter a number.
3. Click **Save**.

**From the Resources list (quick action):**

1. Click the three-dot menu on the deployment's row.
2. Select **Scale**.
3. Enter the desired number of replicas.
4. Click **Scale**.

Setting replicas to 0 stops all pods. The deployment stays defined but no
containers run. The status chip will show **Scaled to 0**.

### Autoscaling

If your cluster has a metrics server installed, you can enable automatic
scaling:

1. Open the deployment's **Edit** dialog.
2. Scroll down to the **Autoscaling** section.
3. Toggle **Enable Horizontal Pod Autoscaler**.
4. Set **Min Replicas** and **Max Replicas**.
5. Choose metric targets:
   - **CPU** -- scale up when average CPU usage exceeds this percentage.
   - **Memory** -- scale up when average memory usage exceeds this percentage.
6. Click **Save**.

Kubernetes will automatically add or remove replicas to keep usage near your
target percentages.

## Restarting

A restart performs a rolling restart -- Kubernetes gradually replaces each pod
with a new one. This is useful when you need to pick up configuration changes
or clear cached state without changing the deployment spec.

**From the deployment detail page:**

1. Click the **Restart** button in the upper right.
2. Confirm in the dialog.

**From the Resources list (quick action):**

1. Click the three-dot menu on the deployment's row.
2. Select **Restart**.
3. Confirm in the dialog.

## Deleting

**From the deployment detail page:**

1. Click the red **Delete** button in the upper right.
2. A confirmation dialog appears. Type the deployment name to confirm.
3. Click **Delete**.

**From the Resources list (quick action):**

1. Click the three-dot menu on the deployment's row.
2. Select **Delete**.
3. Type the deployment name to confirm.
4. Click **Delete**.

Deletion is permanent. All pods are terminated and the deployment is removed
from Kubernetes. If the deployment had GitOps or ingress resources, those are
not automatically cleaned up -- delete them separately if needed.

## Rollback

Every time a deployment changes (image update, env change, probe tweak),
Kubernetes creates a new revision. Deckwatch shows the full revision history
in the **Rollout History** card on the deployment detail page.

Each revision shows the image that was running, when it was created, and
whether it is the current active revision.

To roll back:

1. Find the revision you want to restore in the history table.
2. Click **Roll back to this** on that revision's row.
3. Confirm in the dialog.

The rollback copies the target revision's container spec back onto the
deployment while preserving your current replica count. A new revision is
created with a note indicating it was a rollback.

## Health Probes

Health probes tell Kubernetes how to check whether your container is working.
You configure them in the deployment form under the **Probes** section.

### Liveness Probe

Checks whether the container is still alive. If the liveness check fails
repeatedly, Kubernetes kills the container and starts a new one.

**When to use:** When your application can get into a stuck state where it is
running but not making progress (for example, a deadlock).

### Readiness Probe

Checks whether the container is ready to receive network traffic. While the
readiness check is failing, Kubernetes removes the pod from the service load
balancer so no requests are routed to it.

**When to use:** When your application needs time to load data or warm caches
before it can serve requests.

### Startup Probe

Delays liveness checks until the container finishes starting up. Once the
startup probe succeeds, liveness checks take over.

**When to use:** When your application takes a long time to start (more than
a few seconds). Without a startup probe, the liveness check might kill the
container before it finishes initializing.

### Probe Types

Each probe can use one of three check methods:

| Type | How It Works | Example |
|------|-------------|---------|
| **HTTP GET** | Sends an HTTP request to a path and port. A 200-399 response means success. | Path: `/healthz`, Port: `8080` |
| **TCP Socket** | Tries to open a TCP connection to a port. Success means the port is listening. | Port: `5432` (database) |
| **Exec** | Runs a command inside the container. Exit code 0 means success. | Command: `cat /tmp/healthy` |

### Probe Timing

| Setting | Default | What It Controls |
|---------|---------|-----------------|
| **Initial Delay** | 0s | How long to wait after the container starts before the first check. |
| **Period** | 10s | How often to repeat the check. |
| **Timeout** | 1s | How long to wait for a response before counting the check as failed. |
| **Failure Threshold** | 3 | How many consecutive failures before taking action (kill for liveness, remove from LB for readiness). |
| **Success Threshold** | 1 | How many consecutive successes to require before considering the container healthy again. |

## Resource Limits

Resource requests and limits control how much CPU and memory your containers
can use.

### What "Request" and "Limit" Mean

- **Request** is the amount Kubernetes guarantees your container will get. The
  scheduler uses requests to decide which node to place the pod on.
- **Limit** is the maximum your container can use. If a container exceeds its
  memory limit, Kubernetes kills it (you will see an OOMKilled error). If it
  exceeds its CPU limit, it gets throttled (slowed down, not killed).

### Resource Units

**CPU** is measured in "millicores" (thousandths of a CPU core):

| Value | Meaning |
|-------|---------|
| `100m` | 10% of one CPU core |
| `250m` | 25% of one CPU core |
| `500m` | Half a CPU core |
| `1000m` or `1` | One full CPU core |

**Memory** uses standard byte units:

| Value | Meaning |
|-------|---------|
| `64Mi` | 64 mebibytes (about 67 MB) |
| `128Mi` | 128 mebibytes (about 134 MB) |
| `256Mi` | 256 mebibytes (about 268 MB) |
| `512Mi` | 512 mebibytes (about 537 MB) |
| `1Gi` | 1 gibibyte (about 1.07 GB) |

**Tip:** If you are unsure what to set, start with `100m` CPU request / `250m`
CPU limit and `128Mi` memory request / `256Mi` memory limit. Monitor your
actual usage on the deployment detail page (if metrics-server is installed)
and adjust from there.

## Environment Variables

Environment variables inject configuration into your container at runtime.
Common uses include database connection strings, API keys, feature flags, and
log levels.

In the deployment form:

1. Scroll to the **Environment Variables** section.
2. Click **Add** to create a new row.
3. Enter the **Name** (for example, `DATABASE_URL`) and **Value** (for example,
   `postgres://db:5432/myapp`).
4. Repeat for each variable.

Variables are set on all replicas of the deployment. To change them, edit the
deployment and update the values. Kubernetes will perform a rolling update to
apply the change.

**Note:** For sensitive values like passwords and API keys, consider using
Kubernetes Secrets instead of plain environment variables. You can create
Secrets from the **Secrets** tab on the Resources page.

## Deployment Status Meanings

The status chip on each deployment tells you its current health at a glance:

| Status | Color | What It Means |
|--------|-------|---------------|
| **Available** | Green | All requested replicas are running and passing health checks. Everything is working. |
| **Progressing** | Blue | Kubernetes is actively working on reaching the desired state. This is normal during creation, updates, and scaling. |
| **Degraded** | Yellow | Some replicas are running but not all are healthy. For example, 2 out of 3 replicas are ready. Click the chip to see which pods are having problems. |
| **Failed** | Red | The deployment could not reach a healthy state. Common causes: bad image name, container crashes, insufficient resources. Click the chip for diagnostics. |
| **Scaled to 0** | Grey | The deployment has zero replicas. No pods are running. This is intentional (someone scaled it down) not an error. |

## Pod Conditions

On the deployment detail page, the **Conditions** table shows the Kubernetes
condition checks for the deployment itself. On the Pod Detail page, you will
see pod-level conditions:

| Condition | What It Means |
|-----------|---------------|
| **PodScheduled** | The pod has been assigned to a node. If this is false, the cluster may not have enough resources or the pod has scheduling constraints that cannot be met. |
| **Initialized** | All init containers have completed successfully. If this is false, an init container is still running or has failed. |
| **ContainersReady** | All containers in the pod have passed their readiness checks. If false, one or more containers are not ready. |
| **Ready** | The pod is fully ready to serve traffic. This is the overall "all clear" signal. |

A green check mark next to a condition means it is met. A red X means it is
not -- read the Reason and Message columns for details about what is wrong.

## Viewing YAML

For advanced users, you can view the full Kubernetes YAML spec:

1. On the deployment detail page, click **View YAML**.
2. A dialog opens showing the raw Deployment resource in YAML format.

This is read-only. To edit via YAML, use the YAML tab in the Edit dialog.

## Cloning and Promoting

You can clone a deployment to another namespace using the promote/clone
feature. This copies the deployment's spec into a different namespace, which
is useful for promoting from staging to production.
