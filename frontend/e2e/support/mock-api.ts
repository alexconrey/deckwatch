import type { Page, Request, Route } from "@playwright/test";

/**
 * In-browser mock of the deckwatch HTTP API.
 *
 * One `page.route` catches every same-origin `/api/**` request and dispatches
 * it to a handler table. Handlers are matched by method and path (the query
 * string never takes part in matching, read it from `req.query`). Handlers
 * registered later win, so a test overrides a default with `mockApi.on(...)`.
 *
 * Every request is recorded in `calls`, so tests can assert what the UI sent.
 * A request with no handler is answered 501 and recorded in `unhandled`; the
 * fixture fails the test when that list is not empty. That keeps the suite
 * deterministic: a new API call in the app must come with mock data, instead
 * of rendering whatever error state the missing endpoint produces.
 *
 * Requests to any other origin are aborted and recorded in `external`
 * (also a test failure), unless a handler was registered for that URL with
 * `onExternal`.
 */

export interface MockRequest {
  method: string;
  /** Path below `/api`, e.g. `/namespaces/default/deployments`. */
  path: string;
  /** Values of the `:params` in the matched route pattern (URL-decoded). */
  params: Record<string, string>;
  query: URLSearchParams;
  /** Parsed JSON body, the raw text if it is not JSON, or `null`. */
  body: unknown;
  headers: Record<string, string>;
}

export interface MockResponse {
  status?: number;
  /** Serialized as the JSON body. */
  json?: unknown;
  /** Raw body, used when `json` is absent. */
  body?: string;
  contentType?: string;
  headers?: Record<string, string>;
}

/**
 * Returned by a handler to leave the request pending forever. Used for
 * server-sent event streams: a fulfilled stream ends, and the app then shows
 * "Reconnecting..." and replays the stream, which is not deterministic.
 */
export const HOLD = Symbol("hold");

export type MockResult = MockResponse | typeof HOLD;
export type MockHandler = (req: MockRequest) => MockResult | Promise<MockResult>;

export interface ApiCall {
  method: string;
  path: string;
  query: URLSearchParams;
  body: unknown;
  headers: Record<string, string>;
}

interface RouteEntry {
  method: string;
  regex: RegExp;
  keys: string[];
  source: string;
  handler: MockHandler;
}

/** `/a/:b/c/:d+` -> regex; `:x` matches one segment, `:x+` one or more. */
function compile(pattern: string): { regex: RegExp; keys: string[] } {
  const keys: string[] = [];
  const source = pattern
    .split("/")
    .map((segment) => {
      const m = /^:(\w+)(\+)?$/.exec(segment);
      if (!m) return segment.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      keys.push(m[1]);
      return m[2] ? "(.+)" : "([^/]+)";
    })
    .join("/");
  return { regex: new RegExp(`^${source}$`), keys };
}

function parseBody(request: Request): unknown {
  const raw = request.postData();
  if (raw === null || raw === "") return null;
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

export class MockApi {
  readonly calls: ApiCall[] = [];
  readonly unhandled: ApiCall[] = [];
  readonly external: string[] = [];
  /** Set to true in a test that deliberately hits unmocked endpoints. */
  allowUnhandled = false;

  private readonly routes: RouteEntry[] = [];
  private readonly externalRoutes = new Map<string, MockHandler>();
  private inFlight = 0;
  private lastActivity = Date.now();

  constructor(
    readonly page: Page,
    private readonly origin: string,
  ) {}

  /** Install the routes on the page. Call before the first navigation. */
  async install(): Promise<void> {
    await this.page.route(
      (url) => url.origin === this.origin && url.pathname.startsWith("/api/"),
      (route) => this.dispatch(route),
    );
    await this.page.route(
      (url) => url.protocol.startsWith("http") && url.origin !== this.origin,
      (route) => this.dispatchExternal(route),
    );
  }

  /**
   * Register a handler (or a fixed response) for `method` + `path`, where
   * `path` is below `/api` and may contain `:param` / `:param+` segments.
   * `method` "*" matches any method. Later registrations take precedence.
   */
  on(method: string, path: string, handler: MockHandler | MockResponse): this {
    const { regex, keys } = compile(path);
    this.routes.push({
      method: method.toUpperCase(),
      regex,
      keys,
      source: path,
      handler: typeof handler === "function" ? handler : () => handler,
    });
    return this;
  }

  /** Answer requests to another origin (e.g. the marketplace catalog). */
  onExternal(url: string, handler: MockHandler | MockResponse): this {
    this.externalRoutes.set(url, typeof handler === "function" ? handler : () => handler);
    return this;
  }

  /**
   * Raw `page.route` on top of the mock layer, kept for older specs. The
   * handler may call `route.fallback()` to reach the default mocks.
   */
  async override(
    pattern: string | RegExp,
    handler: (route: Route) => Promise<void> | void,
  ): Promise<void> {
    await this.page.route(pattern, handler);
  }

  /** Recorded calls matching `method` and `path` (same syntax as `on`). */
  requests(method: string, path: string): ApiCall[] {
    const { regex } = compile(path);
    const m = method.toUpperCase();
    return this.calls.filter((c) => (m === "*" || c.method === m) && regex.test(c.path));
  }

  /** Resolves with the first matching call, waiting for it if needed. */
  async waitForRequest(method: string, path: string, timeout = 10_000): Promise<ApiCall> {
    const deadline = Date.now() + timeout;
    for (;;) {
      const [hit] = this.requests(method, path);
      if (hit) return hit;
      if (Date.now() > deadline) {
        throw new Error(`Timed out waiting for ${method} /api${path}`);
      }
      await new Promise((r) => setTimeout(r, 25));
    }
  }

  /**
   * Resolves once no mocked request has been in flight for `quietMs`.
   * Held requests (event streams) do not count.
   */
  async waitForQuiet(quietMs = 150, timeout = 15_000): Promise<void> {
    const deadline = Date.now() + timeout;
    while (this.inFlight > 0 || Date.now() - this.lastActivity < quietMs) {
      if (Date.now() > deadline) {
        throw new Error(`API never went quiet (${this.inFlight} request(s) in flight)`);
      }
      await new Promise((r) => setTimeout(r, 20));
    }
  }

  private async dispatch(route: Route): Promise<void> {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname.slice("/api".length);
    const call: ApiCall = {
      method: request.method(),
      path,
      query: url.searchParams,
      body: parseBody(request),
      headers: request.headers(),
    };
    this.calls.push(call);
    this.inFlight += 1;
    this.lastActivity = Date.now();
    try {
      const entry = this.match(call.method, path);
      if (!entry) {
        this.unhandled.push(call);
        await route.fulfill({
          status: 501,
          contentType: "application/json",
          body: JSON.stringify({
            error: "not_mocked",
            message: `No e2e mock for ${call.method} /api${path}`,
          }),
        });
        return;
      }
      const params: Record<string, string> = {};
      const m = entry.regex.exec(path);
      entry.keys.forEach((key, i) => {
        params[key] = decodeURIComponent(m?.[i + 1] ?? "");
      });
      const result = await entry.handler({ ...call, params });
      if (result === HOLD) return;
      await route.fulfill(toFulfill(result));
    } catch (error) {
      // The page may have closed while a request was in flight (e.g. a
      // metrics beacon sent on unload). Anything else is a real failure.
      if (!String(error).includes("closed")) throw error;
    } finally {
      this.inFlight -= 1;
      this.lastActivity = Date.now();
    }
  }

  private async dispatchExternal(route: Route): Promise<void> {
    const url = route.request().url();
    const handler = this.externalRoutes.get(url);
    if (!handler) {
      this.external.push(`${route.request().method()} ${url}`);
      await route.abort("blockedbyclient").catch(() => {});
      return;
    }
    const result = await handler({
      method: route.request().method(),
      path: url,
      params: {},
      query: new URL(url).searchParams,
      body: parseBody(route.request()),
      headers: route.request().headers(),
    });
    if (result === HOLD) return;
    const fulfill = toFulfill(result);
    fulfill.headers = { "access-control-allow-origin": "*", ...fulfill.headers };
    await route.fulfill(fulfill).catch(() => {});
  }

  private match(method: string, path: string): RouteEntry | undefined {
    for (let i = this.routes.length - 1; i >= 0; i--) {
      const r = this.routes[i];
      if ((r.method === "*" || r.method === method) && r.regex.test(path)) return r;
    }
    return undefined;
  }
}

function toFulfill(res: MockResponse): {
  status: number;
  contentType?: string;
  body?: string;
  headers?: Record<string, string>;
} {
  const status = res.status ?? 200;
  if (res.json !== undefined) {
    return {
      status,
      contentType: res.contentType ?? "application/json",
      body: JSON.stringify(res.json),
      headers: res.headers,
    };
  }
  return {
    status,
    contentType: res.contentType ?? (res.body === undefined ? undefined : "text/plain"),
    body: res.body ?? "",
    headers: res.headers,
  };
}

/** JSON helpers for handlers. */
export const ok = (json: unknown): MockResponse => ({ json });
export const created = (json: unknown): MockResponse => ({ status: 201, json });
export const noContent = (): MockResponse => ({ status: 204 });
export const apiError = (status: number, message: string, error = "error"): MockResponse => ({
  status,
  json: { error, message },
});
