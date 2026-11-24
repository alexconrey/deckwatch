// Global test setup for Vitest.
//
// Unmocked network access fails the test that made it:
//
// - fetch, XMLHttpRequest, WebSocket and EventSource are replaced with
//   guards at module load, before anything can stub them, so
//   vi.unstubAllGlobals() puts the guards back rather than the real
//   implementations. A guard throws (fetch rejects) with
//   "Unmocked network request in unit test: <method> <url>".
// - Each guarded request is also recorded, so the test still fails when app
//   code catches the error. Requests made during a test fail it in afterEach.
//   Requests made outside any test, e.g. at import time, fail the file in
//   afterAll.
// - Every test starts from a fetch stub whose default implementation is the
//   guard. Tests mock responses with
//   `vi.mocked(fetch).mockResolvedValue(...)` or the helpers in
//   helpers/mockFetch.ts.
// - Components mounted with @vue/test-utils are unmounted after each test, so
//   their polling, timers and streams stop with the test that started them.
import { enableAutoUnmount } from "@vue/test-utils";
import { afterAll, afterEach, beforeEach, vi } from "vitest";

let inTest = false;
const testLeaks: Error[] = [];
const outsideLeaks: Error[] = [];

function unmocked(api: string, method: string, url: string | URL): Error {
  const where = inTest ? api : `${api}, outside any test, e.g. at import time`;
  const error = new Error(
    `Unmocked network request in unit test: ${method.toUpperCase()} ${String(url)} (${where})`,
  );
  (inTest ? testLeaks : outsideLeaks).push(error);
  return error;
}

function throwLeaks(leaks: Error[]): void {
  const errors = leaks.splice(0);
  if (errors.length === 1) throw errors[0];
  if (errors.length > 1) {
    throw new AggregateError(
      errors,
      `${errors.length} unmocked network requests:\n${errors.map((e) => e.message).join("\n")}`,
    );
  }
}

function guard(name: string, value: unknown): void {
  Object.defineProperty(globalThis, name, { value, writable: true, configurable: true });
}

function unmockedFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const isRequest = typeof input === "object" && "method" in input;
  const method = init?.method ?? (isRequest ? input.method : "GET");
  return Promise.reject(unmocked("fetch", method, isRequest ? input.url : input));
}

guard("fetch", unmockedFetch);
guard(
  "XMLHttpRequest",
  class {
    open(method: string, url: string | URL): never {
      throw unmocked("XMLHttpRequest", method, url);
    }
  },
);
guard(
  "WebSocket",
  class {
    constructor(url: string | URL) {
      throw unmocked("WebSocket", "GET", url);
    }
  },
);
guard(
  "EventSource",
  class {
    constructor(url: string | URL) {
      throw unmocked("EventSource", "GET", url);
    }
  },
);

// happy-dom also makes requests itself, bypassing the globals above:
// navigator.sendBeacon(), window.open() and <link rel="preload">. Its fetch
// interceptor records those and answers with a network-error response.
// Nothing awaits these requests, so rejecting would only add an unhandled
// rejection. (happy-dom 21 already leaves script, stylesheet, iframe and
// image loading off by default.)
type FetchContext = {
  request: { method: string; url: string };
  window: { Response: { error(): unknown } };
};
const { happyDOM } = globalThis as {
  happyDOM?: { settings?: { fetch?: { interceptor: unknown } } };
};
if (happyDOM?.settings?.fetch) {
  happyDOM.settings.fetch.interceptor = {
    beforeAsyncRequest: async ({ request, window }: FetchContext) => {
      unmocked("happy-dom", request.method, request.url);
      return window.Response.error();
    },
    beforeSyncRequest: ({ request }: FetchContext) => {
      throw unmocked("happy-dom", request.method, request.url);
    },
  };
}

beforeEach(() => {
  inTest = true;
  vi.stubGlobal("fetch", vi.fn(unmockedFetch));
});

afterEach(() => {
  inTest = false;
  // Restore spies first. A spy on a stubbed global would otherwise put the
  // stub back after vi.unstubAllGlobals() removed it, and the guard would be
  // gone for the rest of the file.
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  throwLeaks(testLeaks);
});

afterAll(() => {
  throwLeaks(outsideLeaks);
});

// Registered after the afterEach above so it runs before it: vitest runs
// after-hooks in reverse order by default (sequence.hooks: "stack").
enableAutoUnmount(afterEach);
