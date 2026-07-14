# App Templates

Deckwatch ships a small catalog of deployment templates that pre-fill the
`CreateDeployment` form with sensible defaults for common workload shapes.
The goal is to shorten the path from "I want to run X" to a healthy pod
without asking non-engineers to know what a readiness probe is by heart.

## Available templates

| ID            | Name        | Shape                                                                                             |
|---------------|-------------|---------------------------------------------------------------------------------------------------|
| `web-app`     | Web App     | nginx:1.27-alpine, port 80, HTTP readiness probe on `/`, 100m/128Mi requests, 500m/256Mi limits.  |
| `worker`      | Worker      | No port, no ingress, 1 replica, 200m/256Mi requests, 1/512Mi limits.                              |
| `cron-job`    | Cron Job    | Scale-to-zero one-shot with `sh -c` and a placeholder command; convert to a CronJob when stable.  |
| `static-site` | Static Site | nginx serving static assets on port 80 with HTTP readiness on `/`; small resource footprint.      |

## API

`GET /api/templates`

Returns:

```json
{
  "templates": [
    {
      "id": "web-app",
      "name": "Web App",
      "description": "…",
      "icon": "mdi-web",
      "category": "web_app",
      "payload": {
        "name": "",
        "image": "nginx:1.27-alpine",
        "replicas": 1,
        "port": 80,
        "resource_requests": { "cpu": "100m", "memory": "128Mi" },
        "resource_limits":  { "cpu": "500m", "memory": "256Mi" },
        "readiness_probe":  { "probe_type": "httpGet", "path": "/", "port": 80, "initial_delay_seconds": 5, "period_seconds": 10 }
      }
    }
  ]
}
```

The `payload` field is a superset of `CreateDeploymentRequest`, so the
frontend can POST it directly to
`/api/namespaces/{ns}/deployments` after the operator sets a name (and
optionally tweaks the image).

## Frontend flow

1. `/deployments/templates` — `TemplatePickerPage.vue` — card grid.
2. On click, the payload is stashed in `sessionStorage`
   (`deckwatch.template.payload`) and the router navigates to
   `/deployments/create?template=<id>`.
3. `CreateDeploymentPage.vue` reads and clears the stash on mount, then
   passes it into `DeploymentForm` as `initial-values`.
4. From there it behaves like any other create — the operator can still
   flip probes off, adjust resources, or paste a different image.

The `?template=<id>` query string is cosmetic (it lets the URL survive
copy/paste); the actual data lives in `sessionStorage` because embedding
a full pod-template payload in a URL blows past reasonable limits fast.

## Adding a template

Templates live in `src/handlers/templates.rs::catalog()`. Add a
`DeploymentTemplate` entry — pick an MDI icon, describe the workload in
one to two sentences (the frontend renders this on the card), and fill
the `payload` with the same keys that `CreateDeploymentRequest`
accepts. No frontend or router changes required; the picker page is
purely data-driven.

## What's intentionally *not* done here

- **No template versioning.** Adding a field to `DeploymentTemplate`
  doesn't break existing UIs because the payload is `Partial` on the
  frontend, so extra keys are ignored.
- **No per-namespace template overrides.** The catalog is a static
  hard-coded list. A follow-up could load additional templates from a
  ConfigMap; the API shape doesn't need to change.
- **No ingress/service auto-provisioning from a template.** The `web-app`
  and `static-site` templates create only the Deployment. Ingress is
  still a separate step from the DeploymentDetail page — this keeps the
  create path atomic and lets the operator pick the hostname before the
  Ingress goes live.
