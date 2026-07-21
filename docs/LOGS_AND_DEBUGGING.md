# Logs and Debugging

When something goes wrong with a deployment, Deckwatch gives you several tools
to figure out what happened: a built-in log viewer, pod condition indicators,
Kubernetes events, a diagnostics drawer, and AI-powered diagnosis. This guide
walks through each one.

## Log Viewer

The log viewer appears at the bottom of every deployment detail page (as long
as the deployment has at least one pod running). It shows the full log output
from your container, loaded in bulk and then streamed live.

### Selecting a Pod and Container

If your deployment has multiple replicas, each replica runs in its own pod.
Use the **Pod** dropdown to select which pod's logs you want to see.

If a pod has multiple containers (for example, your main app plus a sidecar),
a **Container** dropdown appears. Select the container whose logs you want.

### Controlling History

The **History** dropdown controls how many past log lines are loaded when the
log viewer opens:

| Option | Lines Loaded |
|--------|-------------|
| **Full history** | All available log lines (can be large). |
| **Last 100** | The most recent 100 lines. |
| **Last 500** | The most recent 500 lines. |
| **Last 1000** | The most recent 1000 lines. |

After the initial history loads, new log lines stream in live. You will see
them appear at the bottom as they are produced by the container.

### Searching Logs

The log viewer highlights error-related keywords. Look for lines containing
`error`, `fatal`, `panic`, or `exception` -- these usually point to the root
cause of a problem.

### Downloading Logs

You can copy log content by selecting text in the log viewer and using your
browser's copy function. For large log sets, consider using the MCP
integration (described below) to fetch logs programmatically.

## Understanding Pod Status

On the deployment detail page, the **Pods** table shows the status of every
pod in the deployment. Here is what the columns mean:

| Column | What to Look For |
|--------|-----------------|
| **Phase** | `Running` is healthy. `Pending` means the pod has not started yet. `Failed` and `CrashLoopBackOff` mean something is wrong. |
| **Ready** | A green check mark means the pod is passing its readiness checks and receiving traffic. A red X means it is not ready. |
| **Restarts** | How many times the container has been restarted. A restart count above 0 and climbing means the container keeps crashing. |

Click a pod name to go to the Pod Detail page, which shows per-container
status including the exact state reason (like `CrashLoopBackOff` or
`OOMKilled`).

## Pod Conditions

On the Pod Detail page, each pod has condition indicators:

| Condition | What It Means When False |
|-----------|------------------------|
| **PodScheduled** | The cluster cannot find a node to run the pod. Possible causes: not enough CPU/memory available, node selectors or tolerations do not match any node. |
| **Initialized** | An init container failed. Init containers run before your main container and are used for setup tasks (database migrations, config generation). Check the init container's logs. |
| **ContainersReady** | One or more containers in the pod are not passing their readiness probes. The container may still be starting up, or it may be unhealthy. |
| **Ready** | The pod is not ready to receive traffic. This is the summary condition -- if any of the above are false, Ready will also be false. |

A green check mark means the condition is satisfied. A red X means it is not.

## Common Error States

When a container is not running correctly, Kubernetes reports a state reason.
Here are the most common ones and what to do about them:

### CrashLoopBackOff

**What it means:** Your application is crashing immediately after it starts.
Kubernetes keeps restarting it, but each time it crashes again. The "BackOff"
means Kubernetes is waiting longer between each restart attempt.

**What to do:**
1. Check the logs for the pod. Look for stack traces, missing configuration,
   or "file not found" errors near the beginning of the log output.
2. Make sure your container's entrypoint command is correct.
3. Check that required environment variables are set.
4. If the app needs a database or external service, verify it can connect.

### ImagePullBackOff / ErrImagePull

**What it means:** Kubernetes cannot download the container image you
specified. The image does not exist, the tag is wrong, or the registry
requires authentication that is not configured.

**What to do:**
1. Double-check the image name and tag in the deployment's Edit dialog. A
   common mistake is a typo in the registry URL or tag.
2. If the image is in a private registry, make sure the cluster has the
   correct pull credentials (an `imagePullSecret` configured by your cluster
   administrator).
3. Try pulling the image manually to verify it exists:
   `docker pull <image>:<tag>`

### OOMKilled

**What it means:** Your container used more memory than its memory limit
allows. Kubernetes killed it to protect the node.

**What to do:**
1. Increase the memory limit in the deployment's Edit dialog. See the
   [Deployments guide](DEPLOYMENTS.md) for how resource limits work.
2. Check your application for memory leaks. Look at the memory sparkline on
   the deployment detail page -- if memory usage climbs steadily over time,
   that is a leak.
3. As a starting point, set the memory limit to 2x what your application
   normally uses.

### Pending

**What it means:** The pod has been created but has not been scheduled to a
node yet.

**What to do:**
1. Check the **Events** section on the deployment detail page. Kubernetes
   usually logs a warning explaining why it could not schedule the pod.
2. Common causes:
   - **Insufficient resources:** The cluster does not have enough free CPU or
     memory to satisfy the pod's resource requests. Lower the requests or add
     more nodes.
   - **Node selectors:** The pod requires a specific node label that no node
     has.
   - **Resource quotas:** The namespace has a resource quota and you have
     exceeded it.

### CreateContainerConfigError

**What it means:** The container configuration is invalid. Usually this means
an environment variable references a Secret or ConfigMap that does not exist.

**What to do:**
1. Check the deployment's environment variables for references to Secrets or
   ConfigMaps.
2. Make sure those Secrets and ConfigMaps exist in the same namespace.
3. Check the **Events** section for a specific error message.

### ContainerCannotRun / RunContainerError

**What it means:** The container's entrypoint command failed to start. The
binary might not exist, might not be executable, or a volume mount might be
blocking a required path.

**What to do:**
1. Verify the command and args in the deployment form.
2. Check that the binary exists in the container image at the path specified.
3. If you are mounting volumes, make sure they do not overwrite critical
   directories.

## Diagnostics Drawer

When a deployment is in a **Degraded** (yellow) or **Failed** (red) state,
you can click the status chip to open a diagnostics drawer on the right side
of the screen.

The drawer shows:

- **Overall status** -- the same status chip.
- **Failed Conditions** -- deployment-level conditions that are not met, with
  their reason and message.
- **Failing Containers** -- a list of containers in an error state across all
  pods, with the state reason and a plain-English explanation of what it means
  and what to try.

This is the fastest way to get an overview of what is wrong without reading
through raw logs.

## Events Feed

The **Events** section on the deployment detail page shows recent Kubernetes
events related to your deployment. Events capture important lifecycle moments:

- Image pulls (successful and failed)
- Pod scheduling decisions
- Container starts and stops
- Probe failures
- Scaling actions

Events are sorted newest-first. Warning events (yellow) indicate problems
that need attention.

## Using AI Diagnostics

When a pod is in an error state or its logs contain error messages, a
**Diagnose with AI** button appears in the log viewer. This feature sends
your pod's logs to an AI agent (Claude or Codex) that analyzes the error and
provides a diagnosis.

### How to Use It

1. Open the deployment detail page for a deployment with issues.
2. Select the affected pod in the log viewer.
3. Click **Diagnose with AI** (the button only appears when the pod is in an
   error state or the logs contain `error` or `fatal` keywords).
4. The first time you click, a dialog asks you to choose between **Claude**
   and **Codex**. Your choice is remembered for the browser session.
5. Deckwatch creates a short-lived Kubernetes Job in the same namespace that
   runs the AI agent on your pod's logs.
6. A status indicator shows the diagnosis progress. When complete, the
   result appears in the log viewer area.

### What the AI Sees

The AI agent receives the last 256 KB of your pod's logs along with context
about the pod state. It does not have access to your source code, database,
or other cluster resources -- only the log output.

### Changing the AI Agent

A small "Using Claude" or "Using Codex" chip appears next to the diagnose
button after your first use. Click **Change** to switch agents.

### Requirements

AI diagnostics requires:
- The cluster administrator to configure AI provider API keys as Kubernetes
  Secrets in the target namespace (see [AI Diagnostics](AI_DIAGNOSTICS.md)
  for operator setup).
- The deckwatch deployment to have the agent images configured (set in the
  Helm chart values).

If the diagnose button does not appear and your pod is in an error state,
AI diagnostics may not be configured for your cluster.

## Using MCP with Claude Code

For deeper investigation, you can connect Claude Code (the AI coding
assistant) to your deckwatch instance using MCP (Model Context Protocol).
This lets you ask natural-language questions about your cluster from your
terminal while also having access to your source code.

### Quick Setup

```bash
claude mcp add --transport http deckwatch http://your-deckwatch-url/mcp
```

### Example Questions

Once connected, you can ask Claude Code questions like:

- "Why is my-app in the default namespace crashing?"
- "Show me the last 50 lines of logs for pod nginx-abc123 in staging."
- "What deployments are running in the production namespace?"
- "Are any pods in staging close to their memory limits?"

Claude Code will call the deckwatch MCP tools to fetch deployment status, pod
logs, events, and metrics, then combine that with your local source code to
provide a complete diagnosis.

For full MCP setup instructions, see the [MCP Integration](MCP.md) guide.

## Diagnostic Checklist

When a deployment is not working, follow this checklist in order:

1. **Check the status chip.** If it is yellow or red, click it to open the
   diagnostics drawer.
2. **Read the conditions.** Look for red X marks in the Conditions table
   and read the Reason/Message columns.
3. **Check the events.** Look for Warning events that explain what went wrong.
4. **Read the logs.** Search for `error`, `fatal`, or `panic` in the log
   output.
5. **Check the pod table.** Look for pods with high restart counts or non-
   Running phases.
6. **Use AI diagnostics.** If the above do not explain the issue, click
   **Diagnose with AI** for an automated analysis.
7. **Use MCP with Claude Code.** For complex issues that require correlating
   logs with source code, connect Claude Code via MCP.
