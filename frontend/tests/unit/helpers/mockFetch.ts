import { vi } from "vitest";

export interface MockFetchResponse {
  status?: number;
  body?: unknown;
  contentType?: string;
}

/**
 * Install a fetch mock that returns a JSON body (or text if `contentType` is
 * a text/* variant). Call `expectCalls()` on the returned handle to assert
 * on the request URL/method/body from within a test.
 */
export function mockFetchOnce(response: MockFetchResponse = {}) {
  const status = response.status ?? 200;
  const contentType = response.contentType ?? "application/json";
  const bodyText =
    typeof response.body === "string"
      ? response.body
      : JSON.stringify(response.body ?? {});

  const fetchMock = vi.fn().mockResolvedValue(
    new Response(status === 204 ? null : bodyText, {
      status,
      headers: { "Content-Type": contentType },
    }),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

/**
 * Install a fetch mock that returns a queue of responses in order.
 */
export function mockFetchSequence(responses: MockFetchResponse[]) {
  const fetchMock = vi.fn();
  for (const r of responses) {
    const status = r.status ?? 200;
    const contentType = r.contentType ?? "application/json";
    const bodyText =
      typeof r.body === "string" ? r.body : JSON.stringify(r.body ?? {});
    fetchMock.mockResolvedValueOnce(
      new Response(status === 204 ? null : bodyText, {
        status,
        headers: { "Content-Type": contentType },
      }),
    );
  }
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}
