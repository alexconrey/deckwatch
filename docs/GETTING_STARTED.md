# Getting Started with Deckwatch

Deckwatch is a web-based platform for managing Kubernetes deployments. It lets
you create, update, scale, and delete deployments through your browser -- no
command-line tools required. You can also view logs, set up automated builds
from Git repositories, and diagnose issues with AI assistance.

This guide walks you through the basics: navigating the interface, creating
your first deployment, and checking that it is running correctly.

## Navigating the Interface

When you open Deckwatch, you will see three main areas:

### App Bar (top of the screen)

The app bar is always visible. From left to right, it contains:

- **Deckwatch logo** -- click to return to the Applications page.
- **Namespace selector** -- a dropdown that controls which namespace you are
  working in. Almost everything in Deckwatch is scoped to the selected
  namespace. You can also create a new namespace by selecting "Create
  Namespace..." at the bottom of the dropdown.
- **Applications** -- navigates to the Applications list.
- **Resources** -- navigates to the Resources page (Deployments, CronJobs,
  Secrets, and ConfigMaps).
- **Registry** -- navigates to the embedded OCI registry (if enabled).
- **Cluster** -- shows a cluster-wide overview of nodes.
- **Theme toggle** -- switches between light and dark mode.
- **Settings** (gear icon) -- opens the Settings page.

### Main Content Area

This is where page content appears. It changes based on which page you have
navigated to.

### Footer

A small bar at the bottom with links to the Help documentation (this book)
and the API reference.

## Choosing a Namespace

Before you can do anything, you need to select a namespace. A namespace is a
way Kubernetes groups related resources together. Your cluster administrator
may have already created namespaces for your team.

1. Click the **Namespace** dropdown in the app bar.
2. Select a namespace from the list.
3. If you need a new namespace, select **Create Namespace...** at the bottom,
   enter a name, and click **Create**.

Everything you see and do in Deckwatch (deployments, pods, ingresses, secrets)
is filtered to the selected namespace.

## Your First Deployment

A deployment tells Kubernetes to run one or more copies of a container image.
Here is how to create one:

1. Make sure you have a namespace selected in the app bar.
2. Navigate to **Resources** in the app bar.
3. Click the **Create Deployment** button in the upper right.
4. Fill in the form:
   - **Name** -- a short, descriptive name using lowercase letters, numbers,
     and hyphens (for example, `my-web-app`).
   - **Image** -- the container image to run. For a quick test, use
     `nginx:latest`.
   - **Replicas** -- how many copies to run. Start with `1`.
   - **Ports** -- add a port entry. Set the port to `80` for nginx.
5. Leave the other fields at their defaults for now.
6. Click **Create**.

Deckwatch will create the deployment and redirect you to its detail page. You
should see the deployment appear with a status chip showing its current state.

**Tip:** If you want a pre-configured starting point, click the **Templates**
button at the top of the Create Deployment page. Templates provide sensible
defaults for common application types (web apps, workers, static sites).

## Checking Deployment Health

After creating a deployment, you will land on the Deployment Detail page. Here
is what to look for:

### Status Chip

At the top of the page, a colored chip shows the deployment's overall health:

| Chip | Meaning |
|------|---------|
| **Available** (green) | All replicas are running and ready to serve traffic. |
| **Progressing** (blue) | Kubernetes is working on getting the deployment to its desired state (for example, pulling an image or starting containers). |
| **Degraded** (yellow) | Some replicas are running, but not all of them are healthy. Click the chip to open the diagnostics drawer for details. |
| **Failed** (red) | The deployment could not reach a healthy state. Click the chip to see what went wrong. |
| **Scaled to 0** (grey) | The deployment has zero replicas. No pods are running. |

### Replica Gauge

Next to the status chip, a gauge shows how many replicas are desired, ready,
and available. For example, `1/1 ready` means one replica was requested and one
is running.

### Pods Table

Further down the page, the **Pods** section lists every pod (running instance)
for this deployment. Each pod shows:

- **Name** -- the auto-generated pod name.
- **Phase** -- whether the pod is Running, Pending, Failed, etc.
- **Ready** -- a check mark if the pod passed its readiness checks.
- **Restarts** -- how many times the container has restarted. A high number
  suggests the app is crashing.

Click a pod name to see its full details, including per-container status.

### Conditions Table

The **Conditions** section at the bottom shows Kubernetes condition checks:

- A green check mark means the condition is met.
- A red X means something is wrong -- read the Reason and Message columns
  for details.

## Viewing Logs

The log viewer appears at the bottom of the Deployment Detail page whenever
there is at least one running pod.

1. Select a pod from the **Pod** dropdown.
2. If the pod has multiple containers, select one from the **Container**
   dropdown.
3. Choose how much history to load from the **History** dropdown (Full history,
   Last 100, Last 500, or Last 1000 lines).

Logs load immediately and then stream live in real time. New lines appear at
the bottom as they are produced.

For more details on using logs to troubleshoot problems, see the
[Logs and Debugging](LOGS_AND_DEBUGGING.md) guide.

## Next Steps

Now that you have a deployment running, here are the next things to explore:

- **[Deployments](DEPLOYMENTS.md)** -- learn about editing, scaling, restarting,
  health probes, resource limits, and environment variables.
- **[Applications](APPLICATIONS.md)** -- group related deployments into
  applications for easier management.
- **[Ingress](INGRESS.md)** -- expose your deployment to the network with a
  URL.
- **[Logs and Debugging](LOGS_AND_DEBUGGING.md)** -- understand error states
  and use AI diagnostics.
- **[GitOps User Guide](GITOPS_USER_GUIDE.md)** -- set up automatic builds
  and deployments from a Git repository.
