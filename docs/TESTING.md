# Testing deckwatch

deckwatch has three test suites:

| Suite | Runner | Scope | Cluster required? |
|---|---|---|---|
| Backend unit | `cargo test` | Pure functions in `src/` (kube_ext, error, config, handler helpers) | No |
| Frontend unit | `vitest` | API clients, Pinia stores, composables, formatters | No |
| End-to-end | Playwright | Full app in a real browser | No (mocked) or Yes (live) |

## Backend unit tests

Runs against pure functions — no live `kube::Client` or network I/O.
The test modules build mock `Deployment` / `Pod` / `Node` / `Ingress`
objects using `k8s-openapi` types.

```bash
cargo test --lib          # unit tests only
cargo test --workspace    # everything
cargo test kube_ext       # filter by module
cargo test --lib -- --nocapture   # see println! output
```

### Coverage areas

- **`kube_ext.rs`** — `deployment_phase` (Available/Progressing/Degraded/Failed
  transitions and edge cases including desired=0), `replica_counts`,
  `primary_image`, `deployment_summary`/`_detail` (env/command/args, resource
  limits, probes for httpGet/tcpSocket/exec and string-port fallback,
  conditions), `pod_summary` (readiness aggregation, container states, missing
  status), `ingress_summary`/`_detail` (hosts, addresses fallback, rules,
  TLS), `cronjob_summary` (active count, defaults), `node_summary` (Ready
  status parsing, role extraction from labels, `<none>` fallback).
- **`error.rs`** — `AppError::into_response` mapping for each variant
  and the `kube::Error::Api` code fallback to 502.
- **`config.rs`** — argv/env parsing, `allowed_namespaces()` filter,
  invalid-port rejection.
- **`state.rs`** — `is_namespace_allowed` allow-list logic (empty list =
  allow all, case-sensitive matching).
- **`handlers/deployments.rs`** — `build_probe` (httpGet default port 80,
  tcpSocket, exec, unknown-type), `build_resources`
  (requests-only / limits-only / both / neither).
- **`handlers/addons.rs`** — `catalog()` invariants (unique ids,
  non-empty fields), `build_resources_from_overrides` (per-request
  overrides beat addon defaults, empty specs collapse to None).

### CI integration

Add to CI after linting:

```yaml
- name: Backend unit tests
  run: cargo test --workspace --locked
```

If using Bazel per `build/CLAUDE.md`, prefer:

```bash
bazel test //...
```

The auto-generated `_test` targets from `k2_rust_library` will pick up the
new `#[cfg(test)]` modules automatically.

### Wiring the staged test files

The test source files are staged (not pre-integrated) under
`/tmp/deckwatch-staging/testing/src/`. See
`src/README_INTEGRATION.md` for the one-liner to append to each source
file — no `[dev-dependencies]` are needed.

## Frontend unit tests

Vitest + `@vue/test-utils` + `happy-dom`. Runs entirely in-process; no
browser or backend needed.

```bash
cd frontend
pnpm install
pnpm test                 # one-shot
pnpm test:watch           # watch mode
pnpm test:coverage        # v8 coverage report → coverage/
```

### Coverage areas

- **`api/client.ts`** — URL prefixing, header merging, JSON parsing, 204
  handling, `ApiError` construction on 4xx/5xx.
- **`api/deployments.ts`** — every method's URL/verb/body shape,
  including YAML text endpoints (`getYaml` / `updateYaml` set the right
  Accept/Content-Type headers).
- **`api/{namespaces,pods,nodes,addons}.ts`** — URL and body shape.
- **`stores/namespace.ts`** — fetch success/failure, auto-select-first
  behavior, "preserve existing selection", empty-list handling.
- **`stores/deployments.ts`** — no-op on empty namespace, populates list
  on success, records error message on failure.
- **`composables/usePolling.ts`** — immediate call on mount, interval
  ticks, stop on unmount, manual stop, restart doesn't stack timers
  (verified with `vi.useFakeTimers()`).
- **`utils/format.ts`** — `formatAge` boundaries (null, 0m, 59s → 0m,
  1h, 24h → 1d). NOTE: this util is staged as an extraction of the
  duplicated `formatAge` in the page components. See "Follow-up cleanups".

### Coverage thresholds

`vitest.config.ts` enforces:

- Lines / functions / statements: 70%
- Branches: 60%

These apply only to `src/api/**`, `src/stores/**`, `src/composables/**`,
`src/utils/**`. Vue components are not covered by unit tests (see
Playwright below).

## End-to-end tests

Playwright drives the app in Chromium / Firefox / WebKit. By default the
tests **mock the API surface** (see `e2e/fixtures.ts`) so they run
against a plain `vite` dev server with no backend or Kubernetes cluster
attached.

```bash
cd frontend
pnpm exec playwright install    # first-time browser download
pnpm test:e2e                   # headless
pnpm test:e2e:ui                # interactive UI mode
pnpm exec playwright show-report   # after a run
```

### Live-cluster mode

Point `PLAYWRIGHT_BASE_URL` at a running deckwatch and disable mocks:

```bash
PLAYWRIGHT_BASE_URL=http://deckwatch.example.com \
PLAYWRIGHT_MOCK_MODE=off \
pnpm test:e2e
```

### Coverage areas

- **Navigation** — `/` → `/deployments` redirect, deployment → pod
  drill-down, `/cluster` overview, 404 fallback.
- **CRUD** — create deployment, delete flow (with confirm dialog).
- **Namespace switching** — the switcher triggers a re-fetch of
  namespace-scoped resources.
- **Deployment detail** — replica counts, resource limits/requests,
  restart action.
- **Error states** — 500 from the deployments list, 404 on unknown
  deployment, 403 for forbidden namespaces.

### CI integration

```yaml
- uses: actions/setup-node@v4
  with: { node-version: 20 }
- run: pnpm install --frozen-lockfile
  working-directory: frontend
- run: pnpm exec playwright install --with-deps
  working-directory: frontend
- run: pnpm test:e2e
  working-directory: frontend
- uses: actions/upload-artifact@v4
  if: failure()
  with:
    name: playwright-report
    path: frontend/playwright-report/
```

`playwright.config.ts` spins up `pnpm dev` automatically if
`PLAYWRIGHT_BASE_URL` is unset. In CI, that gives you a self-contained
run.

## Follow-up cleanups (surfaced while writing tests)

1. **Extract duplicated `formatAge`.** The same 8-line function is
   copy-pasted into `DeploymentsPage.vue`, `DeploymentDetailPage.vue`,
   `ClusterOverviewPage.vue`, and `PodDetailPage.vue`. The staged
   `src/utils/format.ts` is where it belongs — the tests are already
   written against that path.
2. **Wire a11y roles on the namespace switcher.** The e2e test for
   namespace switching soft-skips because there's no accessible role on
   the switcher control. Add `role="combobox"` (or use Vuetify's
   `<v-select>` with proper labels) so tests can locate it reliably.
3. **Backend integration tests are not covered here.** Testing the
   actual handler HTTP surface end-to-end requires either
   `kube::Client::try_from(kube::Config::infer())` against a
   throw-away kind cluster, or the `envtest`-style approach with a
   local `kube-apiserver` binary. The pure helpers cover most of the
   correctness surface; consider adding a `#[ignore]`-gated
   `#[tokio::test]` for the full HTTP surface once a kind-in-CI setup
   is standardized.
4. **`useSse.ts` composable was not tested** — no clear consumer in the
   current page components, and `EventSource` mocking requires more
   scaffolding than usePolling. Add tests when the SSE surface has a
   caller.
