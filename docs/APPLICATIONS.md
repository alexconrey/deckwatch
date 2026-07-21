# Applications

An application in Deckwatch is a logical grouping of related Kubernetes
resources -- deployments and cron jobs that together form a single product or
service. Think of an application as a project folder: it gives you a
high-level view of everything that belongs together, with a single health
indicator that summarizes the state of all its members.

## When to Use Applications

Applications are optional. You can create and manage deployments directly
from the Resources page without ever creating an application. But applications
become useful when:

- You have multiple deployments that work together (for example, a web
  frontend, an API server, and a background worker).
- You want a single dashboard showing overall health across related services.
- You want to link a Git repository at the application level.

## The Applications Page

Navigate to **Applications** in the app bar to see all applications in the
selected namespace. The table shows:

| Column | What It Shows |
|--------|---------------|
| **Name** | The application name. Click to view details. |
| **Description** | A free-text description you provide when creating the app. |
| **Health** | A colored chip summarizing the combined health of all member deployments. |
| **Deployments** | How many deployments belong to this application. |
| **CronJobs** | How many cron jobs belong to this application. |
| **GitOps** | A green check if any member has GitOps enabled, grey otherwise. |
| **Age** | How long ago the application was created. |

If no applications exist in the namespace, you will see a prompt to create
your first one.

## Creating an Application

Deckwatch provides a step-by-step wizard to create an application:

1. Click **Create Application** on the Applications page.
2. **Step 1 -- Basics:**
   - Enter a **Name** for the application (lowercase letters, numbers, and
     hyphens).
   - Optionally add a **Description**.
   - Select the **Namespace** where the application will live. This defaults
     to the namespace currently selected in the app bar, but you can change it.
3. **Step 2 -- Code Source:**
   - Choose **Manual** if you will deploy container images yourself.
   - Choose **Git** if you want to connect a Git repository. Enter the
     **Repository URL**, **Branch**, and **Token Secret** name (see the
     [GitOps User Guide](GITOPS_USER_GUIDE.md) for details on tokens).
4. **Step 3 -- Access:**
   - Choose **Public** if you want to expose the application with a URL
     (creates an ingress). Enter a **Hostname**, **Path**, and **Port**.
   - Choose **Internal** if the application should only be reachable within
     the cluster.
5. **Step 4 -- Review:**
   - Review your choices and click **Create**.

The wizard creates the application record. If you configured Git or public
access, the corresponding ingress resource is also created automatically.

## Application Health

The health chip on each application is derived from the combined status of
its member deployments:

| Health | Color | Meaning |
|--------|-------|---------|
| **Healthy** | Green | All member deployments are in the "Available" state. Everything is working. |
| **Degraded** | Yellow | At least one member deployment is degraded or progressing, but none have failed. |
| **Unhealthy** | Red | At least one member deployment has failed. |
| **Empty** | Grey | The application has no member deployments yet. |

Click an application row to open its detail page and see exactly which
deployments are healthy and which need attention.

## Application Detail Page

The detail page for an application shows:

- **Name, namespace, and description** at the top.
- **Git configuration** (if linked to a repository).
- **Deployments table** -- lists all member deployments with their individual
  status, image, and replica count. Click a deployment name to jump to its
  detail page.
- **CronJobs table** -- lists any cron jobs associated with the application.

## Adding Members

Members are Kubernetes resources (deployments or cron jobs) that belong to an
application. To add a member:

1. Open the application's detail page.
2. Click **Add Member**.
3. In the dialog, select the **Kind** (Deployment or CronJob).
4. Enter the **Resource Name** (the name of an existing deployment or cron
   job in the same namespace).
5. Click **Add**.

The resource now appears in the application's member tables and its health
contributes to the application's overall health indicator.

## Editing an Application

From the application detail page, you can update:

- **Description** -- edit the free-text description.
- **Git configuration** -- add, change, or remove the linked Git repository.

Click the edit controls on the detail page to make changes.

## Deleting an Application

Deleting an application removes the application record but does not delete
the underlying deployments or cron jobs. Those resources continue running
independently.

To delete:

1. Open the application's detail page.
2. Click **Delete**.
3. Confirm in the dialog.

## Linking Deployments to Applications

There are two ways to associate a deployment with an application:

1. **During application creation** -- the wizard can create an initial
   deployment from a template as part of the application setup.
2. **After creation** -- use the **Add Member** button on the application
   detail page to link existing deployments.

A deployment can belong to at most one application. If you need to reorganize,
remove the deployment from one application before adding it to another.
