# deckwatch e2e tests

Playwright tests that drive the deckwatch frontend end-to-end.

## Two modes

**Mocked (default)** — every `/api/**` route is intercepted by
`fixtures.ts`, returning canned deployment / pod / node data. Tests run
against a plain `vite` dev server with no backend or cluster required.
This is what `pnpm test:e2e` does out of the box.

**Real backend** — set `PLAYWRIGHT_BASE_URL` to a running deckwatch
instance and `PLAYWRIGHT_MOCK_MODE=off` to bypass the mocks and drive a
live cluster:

```bash
PLAYWRIGHT_BASE_URL=http://localhost:8080 \
PLAYWRIGHT_MOCK_MODE=off \
pnpm test:e2e
```

## Structure

- `fixtures.ts` — extended Playwright test with default API mocks
- `navigation.spec.ts` — routing, redirects, page mounts
- `deployments-crud.spec.ts` — create/delete flows
- `deployment-detail.spec.ts` — detail page rendering + actions
- `namespace.spec.ts` — namespace switcher wiring
- `error-states.spec.ts` — 4xx/5xx behavior

## Writing new tests

Use the `test` and `expect` exports from `./fixtures` — not from
`@playwright/test` directly. The `mockApi` fixture lets you override any
route for a single test:

```ts
import { test, expect } from "./fixtures";

test("scale action", async ({ page, mockApi }) => {
  await mockApi.override(/\/scale$/, async (route) => {
    return route.fulfill({ status: 200, body: "{}" });
  });
  // ...
});
```
