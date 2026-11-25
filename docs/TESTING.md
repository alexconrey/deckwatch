# Testing deckwatch

deckwatch has ~950+ tests across three test suites:

| Suite | Runner | Approx. count | Scope | Cluster required? |
|---|---|---|---|---|
| Backend unit | `cargo test` | ~350+ | Pure functions in `src/` (kube_ext, error, config, handler helpers, database models) | No |
| Frontend unit | `vitest` | ~200+ | API clients, Pinia stores, composables, formatters | No |
| End-to-end | Playwright | ~400 | Full app in a real browser: functional specs and visual regression | No (mocked) or Yes (live) |

## Backend unit tests

Runs against pure functions — no live `kube::Client` or network I/O.
The test modules build mock `Deployment` / `Pod` / `Node` / `Ingress`
objects using `k8s-openapi` types.

### Test infrastructure

Backend tests use the `#[cfg(test)] #[path = "..."]` pattern to keep
test modules in separate files alongside the source they exercise. This
keeps the main source files clean while allowing tests full access to
private items.

Dev-dependencies used by tests:

- `rand` -- generating randomized test fixtures
- `tempfile` -- ephemeral directories for filesystem-backed tests (e.g.
  registry storage, SQLite databases)

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

### CI integration

The CI pipeline runs the following checks in order:

1. **`cargo check`** -- fast type-check of the entire workspace
2. **`cargo clippy -- -D warnings`** -- lint with all warnings as errors
3. **`cargo fmt --check`** -- enforce consistent formatting
4. **`cargo test --workspace --locked`** -- run all backend tests
5. **Frontend build** -- `pnpm install --frozen-lockfile && pnpm build`
6. **Helm lint** -- `helm lint helm/deckwatch`

```yaml
- name: Backend checks
  run: |
    cargo check --workspace --locked
    cargo clippy --workspace --locked -- -D warnings
    cargo fmt --all -- --check
    cargo test --workspace --locked

- name: Frontend build
  working-directory: frontend
  run: |
    pnpm install --frozen-lockfile
    pnpm build

- name: Helm lint
  run: helm lint helm/deckwatch
```

If using Bazel per `build/CLAUDE.md`, prefer:

```bash
bazel test //...
```

The auto-generated `_test` targets from `k2_rust_library` will pick up the
`#[cfg(test)]` modules automatically.

## Frontend unit tests

Vitest + `@vue/test-utils` + `happy-dom`. Runs entirely in-process; no
browser or backend needed.

### Test infrastructure

- **vitest** -- test runner configured via `vitest.config.ts`
- **happy-dom** -- lightweight DOM implementation (faster than jsdom)
- **@vue/test-utils** -- mount/shallow-mount Vue components with full
  reactivity
- **Vuetify inlining** -- tests import and install Vuetify as a plugin
  in the test setup so components render with the real Vuetify component
  tree instead of stubs. This catches prop-type and slot-shape
  regressions that stub-based tests would miss.

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
- **`api/{namespaces,pods,nodes,plugins}.ts`** — URL and body shape.
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

Playwright drives a production build of the frontend (`vite build`,
served by `vite preview`) in Chromium. Every `/api/**` request is
answered in the browser from deterministic mock data, so no backend or
cluster is needed. There are two kinds of specs in `frontend/e2e/`:

| Specs | Location | What they check |
|---|---|---|
| Functional | `e2e/*.spec.ts` | Navigation, forms, dialogs, tables, the exact requests the UI sends, keyboard/focus basics |
| Visual regression | `e2e/visual/*.spec.ts` | A screenshot of every route and key UI state, compared pixel by pixel with the baselines in `e2e/__screenshots__/` |

The visual specs cover every route in the dark and light themes, with
dialogs, menus and autocompletes open, validation errors, tables with
data and empty, the diagnostics drawer, and the snackbars and toasts.
Key pages are also captured at 1200px and 900px wide. Their job is to
show any unintended change in what users see. A dependency upgrade
such as a Vuetify major should pass against the existing baselines
unchanged.

### Running it locally

The visual baselines are only valid in the official Playwright Docker
image (see "Why a Docker image" below). Run the whole suite there,
exactly as CI does:

```bash
cd frontend
pnpm test:e2e:docker                                # functional + visual
pnpm test:e2e:docker --project=visual               # visual only
pnpm test:e2e:docker e2e/visual/settings.spec.ts    # one file
pnpm test:e2e:docker -g "namespace switcher"        # by title
```

`scripts/e2e-docker.sh` derives the image tag from the
`@playwright/test` version in `pnpm-lock.yaml`
(`mcr.microsoft.com/playwright:v<version>-noble`, forced to
`linux/amd64`). It mounts `frontend/`, keeps a Linux `node_modules` in a
Docker volume, and passes its arguments to `playwright test`. It needs
Docker and nothing else. On Apple silicon the image runs under Rosetta.

The functional specs also run directly on the host. Outside the image the
visual specs skip themselves:

```bash
cd frontend
pnpm exec playwright install chromium   # once
pnpm test:e2e                           # functional specs
pnpm test:e2e:ui                        # interactive UI mode
pnpm exec playwright show-report        # HTML report of the last run
E2E_ALL_BROWSERS=1 pnpm test:e2e        # also Firefox and WebKit
```

`E2E_VISUAL_FORCE=1` runs the visual specs on the host anyway, which is
handy for looking at a page while writing a spec. Their screenshots then
go to `frontend/.e2e-local/` (git-ignored) and never touch the baselines.

### Updating the baselines

A visual failure means the UI now renders differently from the
baseline. The HTML report (and the `playwright-report` CI artifact)
shows the expected, actual and diff images side by side.

**Update baselines only for an intentional UI change, and review them in
the PR.** For example: you changed a component's layout, added a field to a
form, changed the copy, or adopted a new design.

- Never update baselines to make a failure go away when the change was
  not intended, for example after a dependency upgrade (Vuetify, Vite,
  Playwright). A diff there is either a regression to fix in code or a
  deliberate change to call out in the PR.
- Commit only the PNGs your change explains. The reviewer checks each
  changed image (GitHub renders image diffs in "Files changed").

```bash
cd frontend
pnpm test:e2e:update-baselines                       # all visual specs
pnpm test:e2e:update-baselines e2e/visual/settings.spec.ts
git status e2e/__screenshots__                       # review, then commit
```

`--update-snapshots` only rewrites screenshots that no longer match (and
writes missing ones). Deleted or renamed states leave orphaned PNGs
behind, so remove those by hand.

**Bumping Playwright** changes the browser, so it changes rendering.
Bump `@playwright/test` and the `e2e` job's container image in
`.github/workflows/ci.yml` in the same PR. CI fails with a clear message
if they differ. Then regenerate the baselines with the new image and
check that the diffs are only rendering noise.

**Bumping Vite** can change rendering too. Vite 8 minifies CSS with
Lightning CSS, which shortens numbers to six significant digits
(`33.3333333333%` becomes `33.3333%`, `.0892857143em` becomes
`.0892857em`). Those values move some columns and labels by a fraction of
a pixel. Built with Vite 6.4.6 instead of 8.3.8, 190 of the suite's 328
screenshots differ, each by glyphs shifted less than a pixel (at most 409
changed pixels). The baselines come from the Vite 8 build, so treat a
build-tool upgrade like a Playwright bump: regenerate the baselines in
that PR and check that each diff is only rendering.

### Why a Docker image

Screenshots depend on fonts, font hinting and the browser build.
`mcr.microsoft.com/playwright:v<version>-noble` pins all three, and
the CI job runs in the same image. The baselines were rendered under
`linux/amd64`, the architecture of GitHub's runners. With Playwright
1.65.1, the native `linux/arm64` image produced identical screenshots
for the whole suite, but amd64 remains the reference. `e2e/support/reference-image.ts` detects
the image from `/ms-playwright/.docker-info` and checks that its Playwright
version matches the installed one.

### How the runs stay deterministic

- **Data:** `e2e/support/data.ts` is a fixed dataset, and each test gets a
  fresh copy. `e2e/support/routes.ts` serves every endpoint the
  frontend calls. An unmocked `/api` request, a request to any other
  origin, or an uncaught page error fails the test, so a new API call
  must come with mock data.
- **Time:** the browser clock is frozen at `FIXED_NOW` with
  `page.clock.setFixedTime`, so relative ages ("3h", "2d ago") never
  change. Timers still run; specs that need polling to happen (e.g. two
  metric samples for a sparkline) advance them with
  `app.runTimers(ms)`. While a screenshot is taken the timers are paused,
  so a poll cannot flash a progress bar into it. A spec that captures
  something that closes itself (a snackbar, a cluster-alert toast) calls
  `app.freezeTimers()` as soon as it is visible, so a slow machine can't
  let it time out before the capture.
- **Environment:**
  - viewport 1440x900 (1200 and 900 for the responsive shots);
  - `deviceScaleFactor` 1;
  - locale `en-US`, timezone `UTC`;
  - `reducedMotion: reduce`.
- **Theme:** seeded through `localStorage`, so the OS colour scheme
  doesn't matter.
- **Motion:** CSS transitions, animations and click ripples are switched
  off, and screenshots use `animations: "disabled"` and a hidden caret.
- **Settling** (`app.settle()`) waits for:
  - the page's in-flight fetch count to reach zero and the mocked API to
    go quiet;
  - web fonts to load;
  - running animations and Vue transitions to finish.
- **Pointer:** `app.snap()` parks the mouse before each shot, so no hover
  state is captured.
- **Masked text:** the footer's version string is replaced with
  `v0.0.0`, so a release doesn't change every screenshot.
- **Full pages:** the app scrolls `<body>`, not the document, so
  `fullPage` shots grow the viewport to the content height instead of
  using Playwright's `fullPage`.
- **Comparison is strict:** pixelmatch `threshold` 0.05 with
  `maxDiffPixels: 0`. A 1px shift, a changed shadow or a colour change of
  a few levels fails the test; only anti-aliasing is ignored.

### Writing specs

Import `test` and `expect` from `e2e/fixtures.ts`, or from
`e2e/visual/visual-test.ts` for visual specs. Every test gets these
fixtures:

- `app`:
  - `app.goto(path)` navigates and waits for the page to settle.
  - `app.snap(name, { fullPage })` compares a screenshot. The theme and
    the viewport width (if it isn't 1440) are appended to the file name.
  - `app.runTimers(ms)` advances the timers.
  - `app.dialog(title)` finds an open dialog.
  - `app.namespaceSwitcher()` returns the namespace switcher.
- `mockApi`:
  - `mockApi.on(method, path, handler)` overrides one endpoint for one
    test, e.g. `mockApi.on("GET", "/namespaces/:ns/deployments",
    apiError(500, "boom"))`.
  - `mockApi.waitForRequest(method, path)` returns the request the UI
    sent; assert on its `body` and `query`.
  - `mockApi.requests(method, path)` lists the recorded calls.
- `mockState`: the per-test copy of the dataset. Mutate it before
  `app.goto` to set up a state (an empty list, a disabled feature).

`e2e/support/ui.ts` has locators for Vuetify widgets: `field`,
`openMenu`, `choose`, `menuItem`, `card`, `row`. The same specs run
against Vuetify upgrades, so find elements by role, label and text, and
otherwise only by component root classes (`.v-field`, `.v-card`,
`.v-list-item`, `.v-overlay--active`).

In visual specs, wrap tests in `eachTheme(() => ...)` to capture both
themes. Use `fullPage: true` for whole routes, and the default viewport
shot for dialogs and menus.

### Live-cluster mode

Point `PLAYWRIGHT_BASE_URL` at a running deckwatch and disable the
mocks. This runs the functional specs only; the visual specs need the
mocks and skip:

```bash
PLAYWRIGHT_BASE_URL=http://deckwatch.example.com \
PLAYWRIGHT_MOCK_MODE=off \
pnpm test:e2e
```

### CI integration

The `e2e` job in `.github/workflows/ci.yml`:

1. Runs in the `mcr.microsoft.com/playwright:v<version>-noble` container,
   with `--ipc=host` for Chromium's shared memory. It first checks that
   the image matches `@playwright/test` in the lockfile.
2. Restores the pnpm store. Only pushes to `main` save it.
3. Installs dependencies, then runs `pnpm exec playwright test`, which
   builds the app and serves it with `vite preview`.
4. On failure, uploads the `playwright-report` artifact for 14 days. It
   contains the HTML report with the expected, actual and diff images,
   plus `test-results/` with the traces and videos.

Download the artifact, then open the report with
`pnpm exec playwright show-report path/to/playwright-report`, and a trace
with `pnpm exec playwright show-trace path/to/trace.zip`.

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
