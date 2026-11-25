import type { ApplicationDetail, DeploymentDetailResponse, PluginStatus } from "@/types/api";
import { FIXED_NOW, ago, deploymentYaml, manifestFor, mcpActivity, type MockState } from "./data";
import { HOLD, type MockApi, apiError, created, noContent, ok } from "./mock-api";

/**
 * Default handlers for every endpoint the frontend calls, backed by a
 * per-test MockState. Mutating endpoints update the state where a later GET
 * would show the change (namespaces, settings, applications); the rest answer
 * with a plausible response and leave assertions to the test, which can read
 * the request from `mockApi.calls`.
 */
export function installDefaultRoutes(api: MockApi, state: MockState): void {
  const notFound = (what: string) => apiError(404, `Resource not found: ${what}`, "not_found");
  const findDeployment = (ns: string, name: string): DeploymentDetailResponse | undefined =>
    state.deployments.find((d) => d.namespace === ns && d.name === name);
  const findApp = (ns: string, name: string): ApplicationDetail | undefined =>
    state.applications.find((a) => a.namespace === ns && a.name === name);
  const withPluginEvents = () => (state.writePluginEvents.length ? { plugin_events: state.writePluginEvents } : {});
  const summary = (d: DeploymentDetailResponse) => ({
    name: d.name,
    namespace: d.namespace,
    image: d.image,
    replicas: d.replicas,
    status: d.status,
    created_at: d.created_at,
    labels: d.labels,
    resource_requests: d.resource_requests,
  });

  // ---- app shell ---------------------------------------------------------
  api.on("GET", "/settings", () => ok(state.settings));
  api.on("PUT", "/settings", (req) => {
    state.settings = { ...state.settings, ...(req.body as object) };
    return ok(state.settings);
  });
  api.on("POST", "/settings/credentials", (req) => {
    const body = req.body as Record<string, string>;
    const creds = { ...(state.settings.credentials ?? { anthropic_api_key: null, gcp_sa_key: null }) };
    for (const key of ["anthropic_api_key", "gcp_sa_key"] as const) {
      if (key in body) creds[key] = body[key] ? "configured" : null;
    }
    state.settings.credentials = creds;
    return ok(creds);
  });
  api.on("GET", "/settings/outbound", () => ok(state.outbound));
  api.on("POST", "/notifications/test", noContent());
  api.on("GET", "/features", () => ok(state.features));
  api.on("POST", "/frontend-metrics", noContent());
  api.on("GET", "/namespaces", () => ok({ namespaces: state.namespaces }));
  api.on("POST", "/namespaces", (req) => {
    const body = req.body as { name: string; labels?: Record<string, string> };
    if (!state.namespaces.includes(body.name)) state.namespaces.push(body.name);
    return created({ name: body.name, created_at: FIXED_NOW.toISOString(), labels: body.labels ?? {} });
  });
  api.on("GET", "/events", () => ok({ events: state.events }));
  api.on("GET", "/namespaces/:ns/events", (req) => {
    const involved = req.query.get("involved_object");
    const events = state.events.filter(
      (e) =>
        (e.involved_object_namespace ?? e.namespace) === req.params.ns &&
        (!involved || e.involved_object_name === involved || e.involved_object_name.startsWith(`${involved}-`)),
    );
    return ok({ events });
  });

  // ---- applications ------------------------------------------------------
  api.on("GET", "/namespaces/:ns/applications", (req) =>
    ok({
      applications: state.applications
        .filter((a) => a.namespace === req.params.ns)
        .map((a) => ({
          name: a.name,
          namespace: a.namespace,
          description: a.description,
          created_at: a.created_at,
          deployment_count: a.deployments.length,
          cronjob_count: a.cronjobs.length,
          health: a.health,
          gitops_enabled: a.git !== null,
        })),
    }),
  );
  api.on("POST", "/namespaces/:ns/applications", (req) => {
    const body = req.body as { name: string; description?: string; git?: ApplicationDetail["git"] };
    const app: ApplicationDetail = {
      name: body.name,
      namespace: req.params.ns,
      description: body.description ?? "",
      created_at: FIXED_NOW.toISOString(),
      updated_at: null,
      git: body.git ?? null,
      deployments: [],
      cronjobs: [],
      health: "empty",
    };
    state.applications.push(app);
    return created(app);
  });
  api.on("GET", "/namespaces/:ns/applications/:name", (req) => {
    const app = findApp(req.params.ns, req.params.name);
    return app ? ok(app) : notFound(`application/${req.params.name}`);
  });
  api.on("PUT", "/namespaces/:ns/applications/:name", (req) => {
    const app = findApp(req.params.ns, req.params.name);
    if (!app) return notFound(`application/${req.params.name}`);
    const body = req.body as { description?: string; git?: ApplicationDetail["git"] };
    app.description = body.description ?? app.description;
    app.git = body.git ?? null;
    app.updated_at = FIXED_NOW.toISOString();
    return ok(app);
  });
  api.on("DELETE", "/namespaces/:ns/applications/:name", (req) => {
    state.applications = state.applications.filter((a) => !(a.namespace === req.params.ns && a.name === req.params.name));
    return noContent();
  });
  api.on("POST", "/namespaces/:ns/applications/:name/members", (req) => {
    const app = findApp(req.params.ns, req.params.name);
    if (!app) return notFound(`application/${req.params.name}`);
    const body = req.body as { kind: string; resource_name: string };
    if (body.kind === "Deployment") {
      const dep = findDeployment(req.params.ns, body.resource_name);
      if (dep) app.deployments.push(summary(dep));
    } else {
      const cron = state.cronjobs.find((c) => c.name === body.resource_name);
      if (cron) app.cronjobs.push(cron);
    }
    return ok(app);
  });
  api.on("DELETE", "/namespaces/:ns/applications/:name/members/:kind/:resource", (req) => {
    const app = findApp(req.params.ns, req.params.name);
    if (!app) return notFound(`application/${req.params.name}`);
    app.deployments = app.deployments.filter((d) => d.name !== req.params.resource);
    app.cronjobs = app.cronjobs.filter((c) => c.name !== req.params.resource);
    return ok(app);
  });
  api.on("GET", "/namespaces/:ns/applications/:name/plugins", (req) =>
    ok(state.applicationPlugins[req.params.name] ?? []),
  );
  api.on("POST", "/namespaces/:ns/applications/:name/plugins/:plugin", (req) => {
    const list = (state.applicationPlugins[req.params.name] ??= []);
    list.push({ plugin_name: req.params.plugin, created_at: FIXED_NOW.toISOString(), is_loaded: true });
    return noContent();
  });
  api.on("DELETE", "/namespaces/:ns/applications/:name/plugins/:plugin", (req) => {
    state.applicationPlugins[req.params.name] = (state.applicationPlugins[req.params.name] ?? []).filter(
      (p) => p.plugin_name !== req.params.plugin,
    );
    return noContent();
  });
  api.on("GET", "/namespaces/:ns/applications/:name/resources", (req) => ok(state.provisioned[req.params.name] ?? []));
  api.on("POST", "/namespaces/:ns/applications/:name/resources/:plugin/:resource", (req) => {
    const res = {
      id: `res-${req.params.resource}`,
      application_id: `app-${req.params.name}`,
      plugin_name: req.params.plugin,
      resource_id: req.params.resource,
      fields: (req.body as { fields: Record<string, string> }).fields,
      state: { endpoint: `${req.params.name}-${req.params.resource}.example.internal` },
      created_at: FIXED_NOW.toISOString(),
      updated_at: FIXED_NOW.toISOString(),
    };
    (state.provisioned[req.params.name] ??= []).push(res);
    return created(res);
  });
  api.on("DELETE", "/namespaces/:ns/applications/:name/resources/:plugin/:resource", (req) => {
    state.provisioned[req.params.name] = (state.provisioned[req.params.name] ?? []).filter(
      (r) => !(r.plugin_name === req.params.plugin && r.resource_id === req.params.resource),
    );
    return ok({ message: "", plugin_events: [], kubernetes_resource_deletions: [] });
  });

  // ---- plugin status (most severe first, like the backend) ---------------
  const HEALTH_RANK: Record<PluginStatus["health"], number> = { failing: 0, degraded: 1, healthy: 2 };
  const statusRows = (keep: (s: PluginStatus) => boolean) =>
    ok(state.pluginStatus.filter(keep).sort((a, b) => HEALTH_RANK[a.health] - HEALTH_RANK[b.health]));
  api.on("GET", "/plugin-status", (req) => {
    const health = req.query.get("health")?.split(",");
    const plugin = req.query.get("plugin");
    const ns = req.query.get("namespace");
    return statusRows(
      (s) => (!health || health.includes(s.health)) && (!plugin || s.plugin === plugin) && (!ns || s.namespace === ns),
    );
  });
  api.on("GET", "/namespaces/:ns/applications/:name/plugin-status", (req) =>
    statusRows((s) => s.namespace === req.params.ns && s.application === req.params.name),
  );
  api.on("GET", "/plugins/:name/status", (req) => statusRows((s) => s.plugin === req.params.name));

  // ---- deployments -------------------------------------------------------
  api.on("GET", "/namespaces/:ns/deployments", (req) =>
    ok({ deployments: state.deployments.filter((d) => d.namespace === req.params.ns).map(summary) }),
  );
  api.on("POST", "/namespaces/:ns/deployments", (req) => {
    const body = req.body as { name: string; image: string; replicas?: number };
    const replicas = body.replicas ?? 1;
    return created({
      ...summary(state.deployments[0]),
      name: body.name,
      namespace: req.params.ns,
      image: body.image,
      replicas: { desired: replicas, ready: 0, available: 0, updated: 0 },
      status: "progressing",
      created_at: FIXED_NOW.toISOString(),
      ...withPluginEvents(),
    });
  });
  api.on("POST", "/namespaces/:ns/deployments/validate", () => ok({ ok: true, errors: [] }));
  api.on("GET", "/namespaces/:ns/deployments/:name", (req) => {
    const dep = findDeployment(req.params.ns, req.params.name);
    return dep ? ok(dep) : notFound(`deployment/${req.params.name}`);
  });
  api.on("PUT", "/namespaces/:ns/deployments/:name", (req) => {
    const dep = findDeployment(req.params.ns, req.params.name);
    if (!dep) return notFound(`deployment/${req.params.name}`);
    Object.assign(dep, req.body as object);
    return ok({ ...dep, ...withPluginEvents() });
  });
  api.on("DELETE", "/namespaces/:ns/deployments/:name", (req) => {
    state.deployments = state.deployments.filter((d) => !(d.namespace === req.params.ns && d.name === req.params.name));
    return noContent();
  });
  api.on("POST", "/namespaces/:ns/deployments/:name/validate", () => ok({ ok: true, errors: [] }));
  api.on("POST", "/namespaces/:ns/deployments/:name/restart", () => ok({ message: "rolling restart initiated" }));
  api.on("POST", "/namespaces/:ns/deployments/:name/scale", (req) => {
    const dep = findDeployment(req.params.ns, req.params.name);
    if (!dep) return notFound(`deployment/${req.params.name}`);
    dep.replicas = { ...dep.replicas, desired: (req.body as { replicas: number }).replicas };
    return ok(dep);
  });
  api.on("GET", "/namespaces/:ns/deployments/:name/yaml", (req) => {
    const dep = findDeployment(req.params.ns, req.params.name);
    return dep
      ? { body: deploymentYaml(dep.name, dep.image), contentType: "text/yaml" }
      : notFound(`deployment/${req.params.name}`);
  });
  api.on("PUT", "/namespaces/:ns/deployments/:name/yaml", (req) => ok(findDeployment(req.params.ns, req.params.name)));
  api.on("PATCH", "/namespaces/:ns/deployments/:name/probes", (req) => ok(findDeployment(req.params.ns, req.params.name)));
  api.on("POST", "/namespaces/:ns/deployments/:name/containers", (req) => ok(findDeployment(req.params.ns, req.params.name)));
  api.on("DELETE", "/namespaces/:ns/deployments/:name/containers/:container", () => noContent());
  api.on("GET", "/namespaces/:ns/deployments/:name/hpa", (req) => {
    const hpa = req.params.ns === "default" ? state.hpa[req.params.name] : undefined;
    return hpa ? ok(hpa) : notFound(`horizontalpodautoscaler/${req.params.name}`);
  });
  api.on("PUT", "/namespaces/:ns/deployments/:name/hpa", (req) => {
    const body = req.body as { min_replicas: number; max_replicas: number; target_cpu_utilization?: number };
    return ok({
      min_replicas: body.min_replicas,
      max_replicas: body.max_replicas,
      target_cpu_utilization: body.target_cpu_utilization ?? null,
      current_cpu_utilization: null,
      target_memory_utilization: null,
      current_memory_utilization: null,
      current_replicas: body.min_replicas,
      desired_replicas: body.min_replicas,
      conditions: [],
    });
  });
  api.on("DELETE", "/namespaces/:ns/deployments/:name/hpa", () => noContent());
  api.on("GET", "/namespaces/:ns/deployments/:name/history", (req) =>
    ok({ revisions: req.params.ns === "default" ? (state.history[req.params.name] ?? []) : [] }),
  );
  api.on("GET", "/namespaces/:ns/deployments/:name/history/:rev/yaml", (req) =>
    // apiFetch parses every response as JSON, so the YAML travels as a JSON string.
    ok(state.revisionYaml[Number(req.params.rev)] ?? ""),
  );
  api.on("POST", "/namespaces/:ns/deployments/:name/rollback", (req) => ok(findDeployment(req.params.ns, req.params.name)));
  api.on("GET", "/namespaces/:ns/deployments/:name/gitops", (req) =>
    ok(
      (req.params.ns === "default" && state.gitops[req.params.name]) || {
        enabled: false,
        config: null,
        last_commit_sha: null,
        last_build_status: null,
        last_build_job: null,
        last_build_time: null,
        last_build_error: null,
      },
    ),
  );
  api.on("PUT", "/namespaces/:ns/deployments/:name/gitops", (req) => {
    const body = req.body as Record<string, unknown>;
    const status = {
      enabled: true,
      config: {
        git_auth_user: "x-access-token",
        ecr_repository: "",
        include_paths: [],
        exclude_paths: [],
        webhook_secret_configured: Boolean(body.webhook_secret),
        ...body,
      },
      last_commit_sha: null,
      last_build_status: null,
      last_build_job: null,
      last_build_time: null,
      last_build_error: null,
    };
    state.gitops[req.params.name] = status as never;
    return ok(status);
  });
  api.on("DELETE", "/namespaces/:ns/deployments/:name/gitops", (req) => {
    delete state.gitops[req.params.name];
    return noContent();
  });
  api.on("POST", "/namespaces/:ns/deployments/:name/gitops/trigger", () =>
    ok({ message: "build started", job_name: "web-build-abc1234", commit_sha: "abc1234def5678" }),
  );
  api.on("GET", "/namespaces/:ns/deployments/:name/gitops/builds", (req) =>
    ok({ builds: req.params.name === "web" ? state.builds : [] }),
  );
  api.on("GET", "/namespaces/:ns/jobs/:job/pods", (req) =>
    ok({ pods: [{ name: `${req.params.job}-amd64-x7k2p`, phase: "Succeeded", job_name: req.params.job, arch: "amd64" }] }),
  );
  api.on("GET", "/namespaces/:ns/deployments/:name/monitor", (req) => {
    const m = req.params.ns === "default" ? state.monitor[req.params.name] : undefined;
    return ok(
      m ?? {
        enabled: false,
        name: req.params.name,
        namespace: req.params.ns,
        port: "",
        path: "/metrics",
        interval: "30s",
        matching_pods: 0,
        unavailable_reason: null,
      },
    );
  });
  api.on("PUT", "/namespaces/:ns/deployments/:name/monitor", (req) => {
    const body = req.body as { port: string; path: string; interval: string };
    const m = { enabled: true, name: req.params.name, namespace: req.params.ns, matching_pods: 1, unavailable_reason: null, ...body };
    state.monitor[req.params.name] = m;
    return ok(m);
  });
  api.on("DELETE", "/namespaces/:ns/deployments/:name/monitor", (req) => {
    delete state.monitor[req.params.name];
    return noContent();
  });
  api.on("GET", "/namespaces/:ns/deployments/:name/pods", (req) =>
    ok({ pods: findDeployment(req.params.ns, req.params.name)?.pods ?? [] }),
  );

  // ---- pods, logs, metrics, diagnostics ----------------------------------
  api.on("GET", "/namespaces/:ns/pods/:pod", (req) => {
    const pod = state.deployments.flatMap((d) => d.pods).find((p) => p.name === req.params.pod);
    return pod ? ok(pod) : notFound(`pod/${req.params.pod}`);
  });
  // Registered after /pods/:pod so the literal "metrics" segment wins.
  api.on("GET", "/namespaces/:ns/pods/metrics", (req) => {
    const selector = req.query.get("label_selector");
    const app = selector?.startsWith("app=") ? selector.slice(4) : null;
    const pods = state.podMetrics.filter(
      (p) => p.namespace === req.params.ns && (!app || p.name.startsWith(`${app}-`)),
    );
    return ok({ pods });
  });
  api.on("GET", "/namespaces/:ns/pods/:pod/logs/history", (req) => {
    const tail = Number(req.query.get("tail_lines")) || undefined;
    const lines = state.logLines(req.params.pod);
    return ok({ lines: tail ? lines.slice(-tail) : lines });
  });
  // The live log stream stays open (and silent) for the whole test.
  api.on("GET", "/namespaces/:ns/pods/:pod/logs", () => HOLD);
  api.on("GET", "/namespaces/:ns/diagnostics", () => ok({ items: state.diagnostics }));
  api.on("POST", "/namespaces/:ns/diagnostics", () => created({ job_name: "diag-web-7k2p9", status: "pending", agent: "claude" }));
  api.on("GET", "/namespaces/:ns/diagnostics/:job", (req) =>
    ok({
      job_name: req.params.job,
      status: "succeeded",
      agent: "claude",
      source_pod: "web-6c9f7d8b5-4xk2p",
      started_at: ago({ m: 1 }),
      completed_at: FIXED_NOW.toISOString(),
      message: null,
    }),
  );
  api.on("GET", "/namespaces/:ns/diagnostics/:job/result", (req) =>
    ok({ job_name: req.params.job, status: "succeeded", output: "Root cause: the upstream API times out after 60s." }),
  );
  api.on("GET", "/namespaces/:ns/diagnostics/:job/stream", () => ({
    contentType: "text/event-stream",
    body: [
      'event: status\ndata: {"phase":"streaming"}\n\n',
      'event: log\ndata: {"line":"Analyzing 24 log lines from web-6c9f7d8b5-4xk2p..."}\n\n',
      'event: log\ndata: {"line":"Root cause: the upstream API times out after 60s."}\n\n',
      'event: done\ndata: {"status":"succeeded"}\n\n',
    ].join(""),
  }));

  // ---- cronjobs ----------------------------------------------------------
  api.on("GET", "/namespaces/:ns/cronjobs", (req) =>
    ok({ cronjobs: state.cronjobs.filter((c) => c.namespace === req.params.ns) }),
  );
  api.on("POST", "/namespaces/:ns/cronjobs", (req) => {
    const body = req.body as { name: string; schedule: string; suspend?: boolean };
    return created({
      name: body.name,
      namespace: req.params.ns,
      schedule: body.schedule,
      suspend: body.suspend ?? false,
      active_count: 0,
      last_schedule_time: null,
      created_at: FIXED_NOW.toISOString(),
      labels: {},
    });
  });
  api.on("GET", "/namespaces/:ns/cronjobs/:name", (req) => {
    const c = state.cronjobs.find((x) => x.namespace === req.params.ns && x.name === req.params.name);
    return c ? ok(c) : notFound(`cronjob/${req.params.name}`);
  });
  api.on("PATCH", "/namespaces/:ns/cronjobs/:name", (req) => {
    const c = state.cronjobs.find((x) => x.namespace === req.params.ns && x.name === req.params.name);
    return c ? ok({ ...c, ...(req.body as object) }) : notFound(`cronjob/${req.params.name}`);
  });
  api.on("DELETE", "/namespaces/:ns/cronjobs/:name", () => noContent());
  api.on("POST", "/namespaces/:ns/cronjobs/:name/trigger", (req) => ok({ job_name: `${req.params.name}-manual-29104` }));
  api.on("GET", "/namespaces/:ns/cronjobs/:name/logs", (req) =>
    ok({
      job_name: `${req.params.name}-29103`,
      pod_name: `${req.params.name}-29103-5xk8q`,
      logs: "2026-06-01T02:00:03Z starting backup of 3 databases\n2026-06-01T02:04:41Z uploaded 2.1 GiB to s3://acme-backups/2026-06-01\n2026-06-01T02:04:42Z done",
    }),
  );

  // ---- secrets, configmaps, service accounts -----------------------------
  const stripData = <T extends { data: unknown }>({ data: _data, ...rest }: T) => rest;
  api.on("GET", "/namespaces/:ns/secrets", (req) =>
    ok({ secrets: state.secrets.filter((s) => s.namespace === req.params.ns).map(stripData) }),
  );
  api.on("GET", "/namespaces/:ns/secrets/:name", (req) => {
    const s = state.secrets.find((x) => x.namespace === req.params.ns && x.name === req.params.name);
    if (!s) return notFound(`secret/${req.params.name}`);
    const reveal = req.query.get("reveal") === "true";
    return ok({ ...s, data: reveal ? s.data : Object.fromEntries(s.keys.map((k) => [k, ""])) });
  });
  api.on("POST", "/namespaces/:ns/secrets", (req) => {
    const body = req.body as { name: string; secret_type?: string; data: Record<string, string> };
    return created({ name: body.name, namespace: req.params.ns, secret_type: body.secret_type ?? "Opaque", keys: Object.keys(body.data), created_at: FIXED_NOW.toISOString(), data: {} });
  });
  api.on("PUT", "/namespaces/:ns/secrets/:name", (req) => ok({ ...(req.body as object), namespace: req.params.ns }));
  api.on("DELETE", "/namespaces/:ns/secrets/:name", () => noContent());
  api.on("GET", "/namespaces/:ns/configmaps", (req) =>
    ok({ configmaps: state.configmaps.filter((c) => c.namespace === req.params.ns).map(stripData) }),
  );
  api.on("GET", "/namespaces/:ns/configmaps/:name", (req) => {
    const c = state.configmaps.find((x) => x.namespace === req.params.ns && x.name === req.params.name);
    return c ? ok(c) : notFound(`configmap/${req.params.name}`);
  });
  api.on("POST", "/namespaces/:ns/configmaps", (req) => {
    const body = req.body as { name: string; data: Record<string, string> };
    return created({ name: body.name, namespace: req.params.ns, keys: Object.keys(body.data), created_at: FIXED_NOW.toISOString(), data: body.data });
  });
  api.on("PUT", "/namespaces/:ns/configmaps/:name", (req) => {
    const body = req.body as { name: string; data: Record<string, string> };
    return ok({ name: body.name, namespace: req.params.ns, keys: Object.keys(body.data), created_at: FIXED_NOW.toISOString(), data: body.data });
  });
  api.on("DELETE", "/namespaces/:ns/configmaps/:name", () => noContent());
  api.on("GET", "/namespaces/:ns/serviceaccounts", (req) =>
    ok({
      service_accounts: state.serviceAccounts
        .filter((s) => s.namespace === req.params.ns)
        .map(({ annotations: _a, ...rest }) => rest),
    }),
  );
  api.on("GET", "/namespaces/:ns/serviceaccounts/:name", (req) => {
    const s = state.serviceAccounts.find((x) => x.namespace === req.params.ns && x.name === req.params.name);
    return s ? ok(s) : notFound(`serviceaccount/${req.params.name}`);
  });
  api.on("POST", "/namespaces/:ns/serviceaccounts", (req) => {
    const body = req.body as { name: string; annotations?: Record<string, string> };
    return created({ name: body.name, namespace: req.params.ns, irsa_role_arn: body.annotations?.["eks.amazonaws.com/role-arn"] ?? null, created_at: FIXED_NOW.toISOString(), labels: {}, annotations: body.annotations ?? {} });
  });
  api.on("PATCH", "/namespaces/:ns/serviceaccounts/:name", (req) => ok({ name: req.params.name, namespace: req.params.ns, ...(req.body as object) }));
  api.on("DELETE", "/namespaces/:ns/serviceaccounts/:name", () => noContent());

  // ---- ingresses ---------------------------------------------------------
  api.on("GET", "/namespaces/:ns/ingresses", (req) =>
    ok({ ingresses: state.ingresses.filter((i) => i.namespace === req.params.ns) }),
  );
  api.on("GET", "/namespaces/:ns/ingresses/:name", (req) => {
    const ing = state.ingresses.find((i) => i.namespace === req.params.ns && i.name === req.params.name);
    return ing ? ok(ing) : notFound(`ingress/${req.params.name}`);
  });
  const ingressFromBody = (ns: string, body: { name: string; host?: string; ingress_class?: string; annotations?: Record<string, string>; paths: { path: string; path_type?: string; service_name: string; service_port: number }[] }) => ({
    name: body.name,
    namespace: ns,
    hosts: body.host ? [body.host] : [],
    ingress_class: body.ingress_class ?? null,
    created_at: FIXED_NOW.toISOString(),
    labels: {},
    addresses: [],
    rules: [{ host: body.host ?? null, paths: body.paths.map((p) => ({ ...p, path_type: p.path_type ?? "Prefix" })) }],
    tls: [],
    annotations: body.annotations ?? {},
  });
  api.on("POST", "/namespaces/:ns/ingresses", (req) => created(ingressFromBody(req.params.ns, req.body as never)));
  api.on("PUT", "/namespaces/:ns/ingresses/:name", (req) => ok(ingressFromBody(req.params.ns, req.body as never)));
  api.on("DELETE", "/namespaces/:ns/ingresses/:name", () => noContent());
  // The deployment page's ingress dialog reads `classes`; Settings reads
  // `ingress_classes` (the backend shape). Serve both.
  api.on("GET", "/ingressclasses", () =>
    ok({
      ingress_classes: state.ingressClasses,
      classes: state.ingressClasses.map((c) => ({ name: c.name, is_default: c.is_default })),
    }),
  );
  api.on("POST", "/ingressclasses", (req) => created({ parameters: null, is_default: false, ...(req.body as object) }));
  api.on("GET", "/ingressclasses/:name", (req) => {
    const c = state.ingressClasses.find((x) => x.name === req.params.name);
    return c ? ok(c) : notFound(`ingressclass/${req.params.name}`);
  });
  api.on("PUT", "/ingressclasses/:name", (req) => ok({ parameters: null, ...(req.body as object) }));
  api.on("DELETE", "/ingressclasses/:name", () => noContent());
  api.on("GET", "/ingress-templates", () => ok(state.ingressTemplates));

  // ---- cluster -----------------------------------------------------------
  api.on("GET", "/nodes", () => ok({ nodes: state.nodes }));
  api.on("GET", "/nodes/metrics", () => ok({ nodes: state.nodeMetrics }));
  api.on("GET", "/storageclasses", () => ok({ storage_classes: state.storageClasses }));
  api.on("POST", "/storageclasses", (req) => created({ mount_options: null, parameters: null, ...(req.body as object) }));
  api.on("GET", "/storageclasses/:name", (req) => {
    const c = state.storageClasses.find((x) => x.name === req.params.name);
    return c ? ok(c) : notFound(`storageclass/${req.params.name}`);
  });
  api.on("PUT", "/storageclasses/:name", (req) => ok({ mount_options: null, parameters: null, ...(req.body as object) }));
  api.on("DELETE", "/storageclasses/:name", () => noContent());

  // ---- templates, plugins, marketplace -----------------------------------
  api.on("GET", "/templates", () => ok({ templates: state.templates }));
  api.on("PUT", "/templates", (req) => {
    state.templates = (req.body as { templates: MockState["templates"] }).templates;
    return ok({ templates: state.templates });
  });
  api.on("GET", "/plugins", () => ok(state.plugins));
  api.on("GET", "/plugins/:name/schema", (req) => ok(state.plugins.find((p) => p.name === req.params.name)?.config_schema ?? []));
  api.on("POST", "/plugins/:name/config", () => noContent());
  api.onExternal(state.marketplaceUrl, () => ok(state.marketplace));

  // ---- git, registry -----------------------------------------------------
  api.on("GET", "/git/branches", (req) => {
    const branches = state.branches[req.query.get("repo_url") ?? ""] ?? ["main"];
    return ok({ branches, default_branch: branches[0] });
  });
  api.on("PUT", "/git-token-secrets", (req) => {
    const body = req.body as { name: string; secret_name: string };
    return ok({ name: body.name, secret_name: body.secret_name, namespace: "deckwatch" });
  });
  api.on("DELETE", "/git-token-secrets/:secret", () => noContent());
  api.on("GET", "/registry/enabled", () => ok({ enabled: state.features.registry }));
  api.on("GET", "/registry/repositories", () =>
    state.features.registry
      ? ok({ repositories: state.registryRepos })
      : apiError(404, "The embedded registry is not enabled", "not_found"),
  );
  api.on("GET", "/registry/repositories/:repo+/tags", (req) =>
    ok({ name: req.params.repo, tags: state.registryTags[req.params.repo] ?? [] }),
  );
  api.on("GET", "/registry/repositories/:repo+/tags/:tag", (req) => {
    const tag = (state.registryTags[req.params.repo] ?? []).find((t) => t.tag === req.params.tag);
    return tag ? ok(manifestFor(req.params.repo, tag)) : notFound(`tag/${req.params.tag}`);
  });
  api.on("DELETE", "/registry/repositories/:repo+/tags/:tag", () => noContent());

  // ---- audit, agent feedback, docs, previews -----------------------------
  api.on("GET", "/audit", (req) => {
    const q = (key: string) => req.query.get(key) || null;
    const since = q("since");
    const rows = state.audit.filter(
      (e) =>
        (!q("source") || e.source === q("source")) &&
        (!q("action") || e.action === q("action")) &&
        (!q("tool") || e.tool === q("tool")) &&
        (!q("client") || e.client_name === q("client")) &&
        (!q("outcome") || e.outcome === q("outcome")) &&
        (!q("resource_type") || e.resource_type === q("resource_type")) &&
        (!q("namespace") || e.namespace === q("namespace")) &&
        (!since || e.timestamp >= since),
    );
    // Newest first, unless order=asc.
    rows.sort((a, b) => b.timestamp.localeCompare(a.timestamp));
    if (q("order") === "asc") rows.reverse();
    // The cursor is an offset here; the real one is opaque.
    const offset = Number(q("cursor") ?? 0);
    const limit = Number(q("limit") ?? 100);
    const items = rows.slice(offset, offset + limit);
    return ok({ items, next_cursor: offset + limit < rows.length ? String(offset + limit) : null });
  });
  api.on("GET", "/prometheus/mcp_activity", (req) =>
    ok(
      state.features.prometheus
        ? mcpActivity(req.query.get("window") ?? "24h")
        : {
            window_seconds: 86400,
            step_seconds: 300,
            calls_per_minute: [],
            apply_manifest_per_minute: [],
            p95_seconds: [],
            tools: [],
            unavailable_reason:
              "prometheus not configured (set PROMETHEUS_URL to the Prometheus that scrapes deckwatch's /metrics)",
          },
    ),
  );
  api.on("GET", "/agent-feedback", () => ok({ items: state.agentFeedback }));
  api.on("PATCH", "/agent-feedback/:id", (req) => {
    const item = state.agentFeedback.find((f) => f.id === req.params.id);
    if (!item) return notFound(`feedback/${req.params.id}`);
    item.status = (req.body as { status: typeof item.status }).status;
    item.reviewed_at = item.status === "pending" ? null : FIXED_NOW.toISOString();
    return ok(item);
  });
  api.on("GET", "/docs/pages", () => ok({ pages: state.docsPages }));
  api.on("GET", "/docs/pages/:slug", (req) => {
    const md = state.docsMarkdown[req.params.slug];
    return md === undefined
      ? apiError(404, `No such page: ${req.params.slug}`, "not_found")
      : { body: md, contentType: "text/markdown" };
  });
  api.on("GET", "/namespaces/:ns/previews", () => ok({ previews: [] }));
  api.on("GET", "/namespaces/:ns/deployments/:name/previews", () => ok({ previews: [] }));
}
