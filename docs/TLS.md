# Outbound TLS, Endpoints and Credential Binding

Deckwatch calls several services over HTTPS: the AI providers, GitHub and
other git hosts, plugin downloads, notification webhooks, Prometheus, tracing
backends, Entra ID and S3. This page covers which certificates those calls
trust, how to add a private CA, which endpoints operators can change, and why
a stored credential only ever goes to the host it was saved for.

## What deckwatch connects to

| Call | Default destination | Changed with | Credential sent |
|------|---------------------|--------------|-----------------|
| Anthropic Messages API | `https://api.anthropic.com` | Settings: `endpoints.anthropic_api_base` | Anthropic API key |
| Vertex AI token exchange | `https://oauth2.googleapis.com/token` | Settings: `endpoints.vertex_token_url` | JWT signed with the GCP service account key |
| Vertex AI `streamRawPredict` | `https://{region}-aiplatform.googleapis.com` | Settings: `endpoints.vertex_api_base` | access token minted from that key |
| Bedrock `InvokeModel` | `https://bedrock-runtime.{region}.amazonaws.com` | Settings: `endpoints.bedrock_endpoint` | SigV4 signature from the pod's AWS identity |
| GitHub REST API (GitOps path filters) | `https://api.github.com` | Settings: `endpoints.github_api_base` | git token |
| Plugin downloads from GitHub | `https://github.com` (releases), `https://raw.githubusercontent.com` (files) | Settings: `endpoints.github_web_base`, `endpoints.github_raw_base` | git token, if the plugin names one |
| Git smart-HTTP (branch lookups, polling) | the repository URL | the GitOps config | git token |
| Plugin downloads from a URL | the plugin's `url` | the plugin config | git token, if the plugin names one |
| Notification webhook | `notifications.webhook_url` | Settings | none (the URL is the secret) |
| Prometheus | `PROMETHEUS_URL` | environment | none |
| Tracing backend | `tracing.query_url` | Settings | none |
| Entra ID JWKS | `login.microsoftonline.com` | `auth.tenant_id` | none |
| S3 registry backend, AWS STS / IMDS | AWS, or `registry.s3.endpoint` | Helm values | pod AWS identity |

Every one of these goes through the clients in `src/outbound/` (S3 and the
AWS credential providers through `object_store`), so they all share the trust
settings below.

## Trust: the system roots, plus your extra CAs

By default deckwatch trusts the operating system's CA store. In the
distroless image that is Debian's bundle at
`/etc/ssl/certs/ca-certificates.crt`.

To trust a private CA, or the CA of a TLS-inspecting proxy, point
`DECKWATCH_EXTRA_CA_FILE` at a PEM file with one or more `CERTIFICATE`
blocks. Those certificates are **added** to the system roots; public services
such as `api.anthropic.com` and `github.com` keep working. They apply to every
outbound call in the table above, including the S3 registry backend and the
AWS credential providers.

The bundle is read once at startup and validated. Deckwatch refuses to start,
with an error naming the file, when it:

- cannot be read;
- contains no `CERTIFICATE` blocks, or a block that is not valid PEM;
- contains a certificate that does not parse as X.509;
- contains a private key (you probably mounted the wrong Secret key).

Comments between the blocks, as in OpenSSL-style bundles, are fine. Other PEM
blocks (CRLs, CSRs) are ignored.

### Helm

```yaml
outbound:
  extraCaBundle:
    configMapName: corp-ca   # or secretName
    key: ca.crt
```

```bash
kubectl -n deckwatch create configmap corp-ca --from-file=ca.crt=corp-root-ca.pem
```

The chart mounts only that key at `/etc/deckwatch/extra-ca/` and sets
`DECKWATCH_EXTRA_CA_FILE`; a missing key keeps the pod from starting. A
cert-manager trust-manager `Bundle` that targets a ConfigMap works as the
source.

### `SSL_CERT_FILE` and `SSL_CERT_DIR`

On Linux these variables **replace** the system store; they do not add to
it. Setting `SSL_CERT_FILE` to your private CA alone breaks every public
endpoint. Use `DECKWATCH_EXTRA_CA_FILE` instead, or point `SSL_CERT_FILE` at a
combined bundle (system roots plus yours). This is true both on the current
TLS stack (OpenSSL) and after the move to rustls described below.

## Configurable endpoints

The AI and GitHub endpoints can be overridden in Settings, for an LLM gateway,
a private or VPC endpoint, GitHub Enterprise Server, or a local mock. They are
stored in the database under `endpoints`; an empty value means the default:

```jsonc
{
  "endpoints": {
    "anthropic_api_base": "",                                  // https://api.anthropic.com
    "vertex_token_url": "",                                    // https://oauth2.googleapis.com/token
    "vertex_api_base": "",                                     // https://{region}-aiplatform.googleapis.com
    "bedrock_endpoint": "https://vpce-0abc.bedrock-runtime.us-east-1.vpce.amazonaws.com",
    "github_api_base": "https://ghe.example.com/api/v3",
    "github_web_base": "https://ghe.example.com",
    "github_raw_base": "https://ghe.example.com/raw"
  }
}
```

Rules, enforced when the value is saved and again when it is used:

- `https://` only. `http://` is accepted for `localhost`, `127.0.0.0/8` and
  `::1`, or for any host when the deployment sets
  `DECKWATCH_ALLOW_INSECURE_ENDPOINTS=true` (Helm:
  `outbound.allowInsecureEndpoints`). That switch is for local development
  and tests, and is an environment variable rather than a setting because
  deckwatch has no authentication yet.
- No username or password in the URL, no query string, no fragment.
- A valid host. `{region}` may appear once in the host of `vertex_api_base`
  and `bedrock_endpoint`; it is replaced with the AI provider region, which
  must be a plain region name (`us-east-1`, `us-central1`).
- Base URLs may carry a path (`https://gateway.example.com/anthropic`); the
  API path is appended to it.

`GET /api/settings` always returns `endpoints`; a `PUT` that omits it keeps
the stored values. Every change is written to the audit log
(`resource_name: endpoints.<field>`). `GET /api/settings/outbound` lists the
defaults, the effective values, the extra CA bundle and the binding state of
every stored credential.

For GitHub Enterprise Server, set all three GitHub endpoints: the web base
decides which repository URLs are GitHub repositories (for path filtering),
the API base serves the compare API, and the web and raw bases serve plugin
downloads.

## Credential binding

Anyone who can reach deckwatch can change these endpoints (there is no
authentication yet). Without a safeguard, pointing the Anthropic endpoint at
another host would make deckwatch send that host the stored API key. So:

- When a credential is saved, deckwatch records the origin
  (`scheme://host[:port]`) of every endpoint it may be sent to:
  - Anthropic API key: the Anthropic API endpoint;
  - GCP service account key: the Vertex token URL and the Vertex API endpoint
    (the access token minted from the key is bound to the same hosts);
  - git tokens (shared tokens in Settings and per-application tokens in
    GitOps configs): the GitHub API, web and raw endpoints.
- Before a request carrying the credential is built, the configured
  endpoint's origin must match the recorded one. If it does not, nothing is
  sent and the error says so, for example: *the stored Anthropic API key was
  saved for api.anthropic.com, but the Anthropic API endpoint is now
  gateway.example.com ... re-enter the Anthropic API key in Settings to use it
  with gateway.example.com*.
- Changing an endpoint never carries existing secrets along: re-enter the
  credential once the new endpoint is saved. The binding is server-managed; a
  settings `PUT` cannot change it.
- Credentials stored before this existed are bound to the default endpoints,
  where they were meant to go. Deckwatch records that binding at startup (and
  on the next settings save); a database migration does the same for
  per-application GitOps tokens.
- Credentials read from Kubernetes Secrets (`deckwatch-anthropic-api-key`,
  `deckwatch-gcp-sa-key`, git token Secrets) are only sent to the default
  endpoints. To use a custom endpoint, save the credential in Settings.
- Bedrock uses the pod's AWS identity (IRSA, pod identity or environment
  credentials), which deckwatch never stores. Those credentials are only sent
  to AWS hosts (`*.amazonaws.com`, `*.amazonaws.com.cn`, `*.api.aws`) or
  localhost.

Binding covers the endpoints on this page. A git token is also sent to the
repository host named in a GitOps config (smart-HTTP and Kaniko) and to a
plugin's `url` source; those URLs are chosen per repository or plugin and are
not bound.

## Timeouts, user agent and proxies

Every request carries `User-Agent: deckwatch/<version>` and uses the proxy
environment variables (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`).
Clients are built once at startup, one per purpose:

| Purpose | Connect | Total |
|---------|---------|-------|
| AI inference (streams) | 10 s | 10 min |
| AI token exchange | 10 s | 30 s |
| Git and GitHub API | 10 s | 60 s |
| Plugin downloads | 10 s | 30 s |
| Notification webhooks | 5 s | 10 s |
| Prometheus and tracing | 5 s | 10 s |
| Registry probes | 5 s | 15 s |
| Entra JWKS | 5 s | 15 s |

## The move to rustls (reqwest 0.13, PR #239)

Today outbound TLS uses reqwest 0.12 with native-tls: OpenSSL on Linux, the
Security framework on macOS. Dependabot PR #239 moves to reqwest 0.13, which
uses rustls with `rustls-platform-verifier` and removes OpenSSL, native-tls
and hyper-tls from the build. What changes:

- **Extra CAs keep working.** The bundle is passed to the platform verifier
  as extra roots next to the OS store. The code calls reqwest's
  `add_root_certificate`, which 0.13 deprecates in favour of
  `tls_certs_merge` (same behaviour); the call is in one place,
  `apply_extra_roots` in `src/outbound/mod.rs`.
- **System roots on Linux** come from the same Debian bundle, read by
  rustls-native-certs. `SSL_CERT_FILE` and `SSL_CERT_DIR` still replace it.
- **No CN fallback.** OpenSSL accepts a server certificate whose host name
  is only in the subject CN; rustls (webpki) does not: the name (or IP) must
  be in the subject alternative names. Certificates from public CAs already
  comply; check certificates issued by a private CA before upgrading.
- `object_store` (S3 registry, Bedrock credentials) already uses reqwest 0.13
  with rustls, so it behaves the same before and after.

`tests/outbound_tls.rs` is the before/after check: TLS mock upstreams on
127.0.0.1 signed by a test CA that is trusted only through
`DECKWATCH_EXTRA_CA_FILE`, covering the request shape of every call above
plus untrusted CA, hostname mismatch, expired certificate, binding and
non-https cases. Run it with:

```bash
cargo test --test outbound_tls
```

It sets process-wide environment variables, which is why it is a separate
test binary. The check that an extra CA does not drop the system roots uses
`SSL_CERT_FILE` as a stand-in for the system store, so it runs on Linux only
(CI); macOS reads its keychain instead.
