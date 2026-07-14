import { ref, computed, readonly } from "vue";
import type { AuthSettings } from "@/types/api";

/**
 * Microsoft Entra (OIDC) authentication composable.
 *
 * Uses the implicit ID-token flow (`response_type=id_token`) so the SPA can
 * validate against the backend without a code-for-token exchange. Entra
 * returns the JWT in the URL fragment on the redirect back to
 * `/auth/callback`, we stash it in sessionStorage, and every `apiFetch` call
 * attaches it as `Authorization: Bearer <jwt>`. The Rust `require_auth`
 * middleware validates signature/issuer/audience/exp against Entra JWKS.
 *
 * When auth is disabled (default) everything is a no-op and
 * `isAuthenticated` reports true so views don't need to gate.
 */

interface AuthUser {
  name: string | null;
  email: string | null;
  groups: string[];
}

const STORAGE_KEY = "deckwatch.auth.token";
const USER_STORAGE_KEY = "deckwatch.auth.user";
const STATE_KEY = "deckwatch.auth.state";
const NONCE_KEY = "deckwatch.auth.nonce";
const RETURN_KEY = "deckwatch.auth.return_to";

const token = ref<string | null>(sessionStorage.getItem(STORAGE_KEY));
const user = ref<AuthUser | null>(readStoredUser());
const authSettings = ref<AuthSettings | null>(null);

function readStoredUser(): AuthUser | null {
  const raw = sessionStorage.getItem(USER_STORAGE_KEY);
  if (!raw) return null;
  try {
    return JSON.parse(raw) as AuthUser;
  } catch {
    return null;
  }
}

function isEnabled(): boolean {
  const s = authSettings.value;
  return !!(s && s.enabled && s.tenant_id && s.client_id);
}

const isAuthenticated = computed<boolean>(() => {
  if (!isEnabled()) return true;
  return token.value !== null;
});

function setAuthSettings(settings: AuthSettings | null | undefined) {
  authSettings.value = settings ?? null;
}

function redirectUri(s: AuthSettings): string {
  return s.redirect_uri || `${window.location.origin}/auth/callback`;
}

function authorizeUrl(returnTo?: string): string | null {
  const s = authSettings.value;
  if (!s || !s.enabled) return null;

  const state = crypto.randomUUID();
  const nonce = crypto.randomUUID();
  sessionStorage.setItem(STATE_KEY, state);
  sessionStorage.setItem(NONCE_KEY, nonce);
  if (returnTo) {
    sessionStorage.setItem(RETURN_KEY, returnTo);
  }

  const params = new URLSearchParams({
    client_id: s.client_id,
    response_type: "id_token",
    redirect_uri: redirectUri(s),
    response_mode: "fragment",
    scope: s.scopes || "openid profile email",
    state,
    nonce,
  });
  return `https://login.microsoftonline.com/${s.tenant_id}/oauth2/v2.0/authorize?${params.toString()}`;
}

async function login(returnTo?: string): Promise<void> {
  const url = authorizeUrl(returnTo);
  if (!url) return;
  window.location.assign(url);
}

interface JwtPayload {
  name?: string;
  email?: string;
  preferred_username?: string;
  groups?: string[];
  roles?: string[];
  nonce?: string;
}

function decodeJwt(jwt: string): JwtPayload | null {
  const parts = jwt.split(".");
  if (parts.length !== 3) return null;
  try {
    const b64 = parts[1].replace(/-/g, "+").replace(/_/g, "/");
    // Pad to a multiple of 4 for atob.
    const padded = b64 + "=".repeat((4 - (b64.length % 4)) % 4);
    return JSON.parse(atob(padded)) as JwtPayload;
  } catch {
    return null;
  }
}

/**
 * Handle the OIDC redirect from Entra. Entra returns the ID token in the URL
 * fragment (`#id_token=...&state=...`). We verify `state` matches what we
 * stashed pre-redirect, verify `nonce` inside the JWT matches what we
 * stashed, then persist the token. Signature/issuer/audience/exp are checked
 * server-side by the Rust middleware — we don't attempt crypto in the SPA.
 */
async function handleCallback(): Promise<{ returnTo: string }> {
  const hash = window.location.hash.startsWith("#")
    ? window.location.hash.slice(1)
    : window.location.hash;
  const params = new URLSearchParams(hash);

  const err = params.get("error");
  if (err) {
    const desc = params.get("error_description") || "";
    throw new Error(`Entra returned ${err}: ${desc}`);
  }

  const idToken = params.get("id_token");
  const state = params.get("state");
  if (!idToken) throw new Error("Entra redirect missing id_token");

  const expectedState = sessionStorage.getItem(STATE_KEY);
  if (!expectedState || state !== expectedState) {
    throw new Error("OIDC state mismatch — refusing redirect");
  }

  const expectedNonce = sessionStorage.getItem(NONCE_KEY);
  const payload = decodeJwt(idToken);
  if (!payload) throw new Error("Malformed id_token");
  if (expectedNonce && payload.nonce !== expectedNonce) {
    throw new Error("OIDC nonce mismatch — refusing redirect");
  }

  token.value = idToken;
  sessionStorage.setItem(STORAGE_KEY, idToken);

  const userObj: AuthUser = {
    name: payload.name ?? null,
    email: payload.email ?? payload.preferred_username ?? null,
    groups: payload.groups ?? [],
  };
  user.value = userObj;
  sessionStorage.setItem(USER_STORAGE_KEY, JSON.stringify(userObj));

  sessionStorage.removeItem(STATE_KEY);
  sessionStorage.removeItem(NONCE_KEY);
  const returnTo = sessionStorage.getItem(RETURN_KEY) || "/";
  sessionStorage.removeItem(RETURN_KEY);
  return { returnTo };
}

function logout(): void {
  const s = authSettings.value;

  token.value = null;
  user.value = null;
  sessionStorage.removeItem(STORAGE_KEY);
  sessionStorage.removeItem(USER_STORAGE_KEY);
  sessionStorage.removeItem(STATE_KEY);
  sessionStorage.removeItem(NONCE_KEY);
  sessionStorage.removeItem(RETURN_KEY);

  if (s && s.enabled && s.tenant_id) {
    const params = new URLSearchParams({
      post_logout_redirect_uri: redirectUri(s),
    });
    window.location.assign(
      `https://login.microsoftonline.com/${s.tenant_id}/oauth2/v2.0/logout?${params.toString()}`,
    );
  }
}

function currentToken(): string | null {
  return token.value;
}

export function useAuth() {
  return {
    isAuthenticated,
    user: readonly(user),
    token: readonly(token),
    isEnabled,
    setAuthSettings,
    login,
    handleCallback,
    logout,
    currentToken,
  };
}
