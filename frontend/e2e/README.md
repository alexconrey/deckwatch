# deckwatch e2e tests

Playwright tests that drive a production build of the frontend with every
`/api/**` request mocked in the browser. The full guide is in
[docs/TESTING.md](../../docs/TESTING.md#end-to-end-tests); this is the map.

```bash
pnpm test:e2e:docker                 # everything, in the pinned Playwright image (as CI)
pnpm test:e2e                        # functional specs on the host (visual specs skip)
pnpm test:e2e:update-baselines       # intentional UI change only, reviewed in the PR
```

## Layout

- `fixtures.ts`: the extended `test`: per-test mock state, frozen clock,
  seeded theme, no motion, and failure on unmocked requests or page errors.
- `support/mock-api.ts`: the `/api/**` router (`mockApi.on`,
  `mockApi.waitForRequest`, `mockApi.requests`).
- `support/data.ts`: the deterministic dataset, relative to `FIXED_NOW`.
- `support/routes.ts`: default handlers for every endpoint.
- `support/app.ts`: `app.goto`, `app.settle`, `app.snap`, `app.runTimers`.
- `support/ui.ts`: locators for Vuetify widgets (selects, menus, cards, rows).
- `support/reference-image.ts`: detects the Playwright Docker image.
- `*.spec.ts`: functional specs (behaviour and request payloads).
- `visual/*.spec.ts`: visual specs; `visual/visual-test.ts` adds
  `eachTheme` and skips outside the Docker image.
- `__screenshots__/`: the committed baselines, one folder per visual spec.

## Writing a test

```ts
import { apiError } from "./support/mock-api";
import { expect, test } from "./fixtures";

test("scale sends the replica count", async ({ app, page, mockApi }) => {
  await app.goto("/deployments");
  // ...drive the UI...
  const call = await mockApi.waitForRequest("POST", "/namespaces/default/deployments/web/scale");
  expect(call.body).toEqual({ replicas: 5 });
});

test("a failing list shows the API message", async ({ app, page, mockApi }) => {
  mockApi.on("GET", "/namespaces/:ns/deployments", apiError(500, "the server is down"));
  await app.goto("/deployments");
  await expect(page.getByRole("alert").filter({ hasText: "the server is down" })).toBeVisible();
});
```

A visual spec captures both themes:

```ts
import { eachTheme, expect, test } from "./visual-test";

eachTheme(() => {
  test("cluster overview", async ({ app }) => {
    await app.goto("/cluster");
    await app.runTimers(10_000); // two metric samples, so sparklines are stable
    await app.snap("cluster/overview", { fullPage: true });
  });
});
```
