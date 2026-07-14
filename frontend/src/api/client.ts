import { recordApiCall, recordError } from "@/composables/useMetrics";
import { useAuth } from "@/composables/useAuth";

const BASE_URL = "/api";

export class ApiError extends Error {
  constructor(
    public status: number,
    public body: { error: string; message: string },
  ) {
    super(body.message);
  }
}

export async function apiFetch<T>(
  path: string,
  options?: RequestInit,
): Promise<T> {
  const method = (options?.method ?? "GET").toUpperCase();
  const started = performance.now();
  let status = 0;

  // Attach the Entra bearer token when the SPA has one. When auth is
  // disabled the token is always null and no header is added, so the
  // backend's no-op middleware still passes the request through.
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...(options?.headers as Record<string, string> | undefined),
  };
  const token = useAuth().currentToken();
  if (token && !headers["Authorization"]) {
    headers["Authorization"] = `Bearer ${token}`;
  }

  try {
    const response = await fetch(`${BASE_URL}${path}`, {
      ...options,
      headers,
    });
    status = response.status;

    // A 401 after we thought we had a session means the token expired or
    // was rejected by the middleware. Wipe it and let the router guard
    // bounce the user back through Entra on the next navigation.
    if (response.status === 401 && useAuth().isEnabled()) {
      useAuth().logout();
    }

    if (!response.ok) {
      const body = await response.json();
      throw new ApiError(response.status, body);
    }

    if (response.status === 204) {
      return undefined as T;
    }

    return (await response.json()) as T;
  } catch (err) {
    // Distinguish transport failures (network) from API-level 4xx/5xx so the
    // dashboard can separate "backend unreachable" from "backend rejected".
    if (err instanceof ApiError) {
      recordError("api");
    } else {
      recordError("network");
    }
    throw err;
  } finally {
    recordApiCall(path, method, status, performance.now() - started);
  }
}
