# Ingress -- Exposing Apps via URLs

An ingress is a Kubernetes resource that maps an external URL to a service
running inside the cluster. Without an ingress, your deployment is only
reachable from within the cluster. With an ingress, users (or other systems)
can access it via a hostname like `myapp.example.com`.

In plain terms: a deployment runs your app, and an ingress gives it a URL.

## How Ingress Works in Deckwatch

Deckwatch manages ingresses directly from the deployment detail page. When
you create an ingress for a deployment, Deckwatch automatically:

1. Creates a Kubernetes Service that routes traffic to your deployment's pods.
2. Creates an Ingress resource that maps a hostname and path to that Service.

You do not need to create Services manually -- Deckwatch handles that for you.

## Creating an Ingress

1. Navigate to the deployment you want to expose.
2. In the **Ingresses** card, click **Add**.
3. Fill in the ingress form:

| Field | What It Means |
|-------|---------------|
| **Ingress Name** | A name for the ingress resource. Defaults to `<deployment>-ingress`. You can change it, but it must be unique within the namespace. |
| **Hostname** | The domain name where your app will be accessible, such as `myapp.example.com`. Leave blank to match any hostname (not recommended for production). |
| **Path** | The URL path to match. Defaults to `/`, which matches all paths. You might use `/api` if only API requests should go to this deployment. |
| **Type** | How the path is matched. `Prefix` matches the path and anything under it (e.g., `/api` also matches `/api/users`). `Exact` matches only the exact path. |
| **Port** | The port on your deployment's Service. This should match the container port your app listens on (for example, `80` for nginx, `8080` for a custom web server). |
| **Ingress Class** | The ingress controller to use. Deckwatch auto-discovers available ingress classes from the cluster and shows them in a dropdown. If your cluster has a default class, it is pre-selected. |

4. Click **Create**.

The ingress appears in the **Ingresses** table on the deployment detail page.

## Understanding Ingress Classes

An ingress class tells Kubernetes which ingress controller should handle the
ingress rule. Different clusters have different controllers installed:

- **nginx** -- the most common ingress controller. Runs an nginx reverse proxy.
- **alb** -- AWS Application Load Balancer. Used on EKS clusters.
- **traefik** -- used by k3s and some development clusters.

Deckwatch reads the available ingress classes from the cluster and shows them
in the dropdown. If your cluster has a default ingress class marked, it is
pre-selected when you create a new ingress.

If the dropdown is empty, your cluster may not have an ingress controller
installed. Talk to your cluster administrator.

## Editing an Ingress

1. In the **Ingresses** table on the deployment detail page, click the
   pencil icon on the ingress row.
2. The same form opens, pre-filled with the current values.
3. Change the hostname, path, port, or ingress class.
4. Click **Save**.

You cannot change the ingress name after creation. If you need a different
name, delete the ingress and create a new one.

## Deleting an Ingress

1. In the **Ingresses** table, click the red trash icon on the ingress row.
2. Confirm in the dialog.

Deleting an ingress removes the routing rule. Your deployment keeps running,
but it is no longer reachable via the hostname.

## TLS Configuration

For HTTPS, your ingress needs a TLS certificate. This is typically handled at
the cluster level by a certificate manager (such as cert-manager) or by the
ingress controller itself (such as AWS ALB with ACM certificates).

If your cluster requires manual TLS configuration, you can set it up through
the Kubernetes API or ask your cluster administrator to configure it.

## Troubleshooting: "My URL Doesn't Work"

If you created an ingress but cannot reach your app, work through these
checks in order:

### 1. Is the deployment running?

Check that the deployment's status chip shows **Available** (green). If the
deployment is failed or degraded, fix that first -- the ingress cannot route
traffic to pods that are not running.

### 2. Is the hostname correct?

The hostname in the ingress must have a DNS record pointing to the cluster's
ingress controller. Check with your cluster administrator or DNS provider
that `myapp.example.com` resolves to the right IP address.

### 3. Is the port correct?

The ingress port must match the port your container actually listens on. If
your app runs on port 8080 but the ingress points to port 80, traffic will
not reach it.

How to check: look at the deployment's **Ports** section (visible in the
Edit dialog) and compare with the ingress port.

### 4. Is the ingress class correct?

Make sure you selected an ingress class that exists in your cluster. An
ingress with a non-existent class will be ignored by all controllers.

### 5. Does the ingress have an address?

In the **Ingresses** table, check the **Addresses** column. If it is empty,
the ingress controller has not picked up the ingress yet. This could mean:

- The ingress controller is not running.
- The ingress class does not match any installed controller.
- There is a configuration issue in the controller.

### 6. Check events

Look at the **Events** section on the deployment detail page. Kubernetes
events often include warnings about ingress configuration issues, such as
missing Services or invalid backends.

### 7. Check the pod logs

If the ingress is configured correctly but the app returns errors (502, 503),
the problem is likely in your application. Check the logs on the deployment
detail page for startup errors or crashes.
