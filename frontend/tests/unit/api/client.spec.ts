import { describe, expect, it } from "vitest";
import { ApiError, apiFetch } from "@/api/client";
import { mockFetchOnce } from "../helpers/mockFetch";

describe("apiFetch", () => {
  it("prefixes /api and returns the parsed JSON body on 2xx", async () => {
    const fetchMock = mockFetchOnce({ body: { ok: true, count: 3 } });
    const result = await apiFetch<{ ok: boolean; count: number }>("/deployments");
    expect(fetchMock).toHaveBeenCalledOnce();
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/deployments");
    expect(init?.headers).toMatchObject({ "Content-Type": "application/json" });
    expect(result).toEqual({ ok: true, count: 3 });
  });

  it("returns undefined for 204 No Content", async () => {
    mockFetchOnce({ status: 204 });
    const result = await apiFetch<void>("/foo", { method: "DELETE" });
    expect(result).toBeUndefined();
  });

  it("throws ApiError with the parsed error body on 4xx", async () => {
    mockFetchOnce({
      status: 404,
      body: { error: "not_found", message: "deploy/foo missing" },
    });
    await expect(apiFetch("/foo")).rejects.toBeInstanceOf(ApiError);
    try {
      await apiFetch("/foo");
    } catch (e) {
      const err = e as ApiError;
      expect(err.status).toBe(404);
      expect(err.body.error).toBe("not_found");
      expect(err.message).toBe("deploy/foo missing");
    }
  });

  it("throws ApiError on 5xx too", async () => {
    mockFetchOnce({
      status: 500,
      body: { error: "kube_error", message: "boom" },
    });
    await expect(apiFetch("/foo")).rejects.toMatchObject({ status: 500 });
  });

  it("merges custom headers with the default Content-Type", async () => {
    const fetchMock = mockFetchOnce({ body: {} });
    await apiFetch("/foo", {
      method: "POST",
      headers: { "X-Custom": "yes" },
      body: JSON.stringify({}),
    });
    const [, init] = fetchMock.mock.calls[0];
    expect(init?.headers).toMatchObject({
      "Content-Type": "application/json",
      "X-Custom": "yes",
    });
    expect(init?.method).toBe("POST");
  });
});
