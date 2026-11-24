# Changelog

All notable changes to deckwatch are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/) (within 0.x, a minor release can
contain breaking changes, so read its upgrade notes before upgrading).

A release is the container image `ghcr.io/alexconrey/deckwatch:<version>`
(linux/amd64 and linux/arm64; the image tag has no `v`) plus the Helm chart in
`helm/deckwatch` at the same version, both built from the `v<version>` tag.
The release workflow refuses a tag that doesn't match `Cargo.toml`,
`helm/deckwatch/Chart.yaml` (`version` and `appVersion`),
`frontend/package.json` and a dated section in this file, and publishes that
section as the GitHub Release notes.

Changes merged since the last release are listed under `[Unreleased]`.

## [Unreleased]

## [0.7.0] - 2026-11-24

A security and hardening release. It also moves to kube 5, which requires
Kubernetes 1.33 or newer, makes outbound HTTPS configurable (extra trusted
CAs, and endpoint overrides that stored credentials can't follow), and fixes
several plugin lifecycle bugs. Some changes need action before or after the
upgrade: read the upgrade notes first.

### Upgrade notes

Breaking changes, and other changes that may need action, most important
first.

- **Kubernetes 1.33 or newer is required.** deckwatch builds against kube 5
  and k8s-openapi 0.29 with the Kubernetes 1.33 API (`v1_33`), and the chart
  declares `kubeVersion: ">=1.33.0-0"`, so Helm refuses older clusters. The
  embedded mcp-k8s (now v0.5.0) uses the same API types: its tools for alpha
  and beta APIs (DRA, VolumeAttributesClass, ClusterTrustBundle,
  LeaseCandidate, MutatingAdmissionPolicy, StorageVersion,
  StorageVersionMigration) return the API server's 404 on clusters that don't
  enable those API versions, which is the default. See "404 from DRA,
  VolumeAttributesClass or other alpha/beta tools" in
  [docs/MCP.md](docs/MCP.md). (#252)
- **Back up the database before upgrading.** 0.7.0 migrates it on startup
  (adding `gitops_configs.token_binding`), and deckwatch 0.6.x refuses to
  start on a migrated database (sea-orm reports that migration
  `m20261124_000001_gitops_token_binding` is missing). Rolling back means
  restoring the backup. (#253)
- **Cross-site browser requests are rejected.** The permissive CORS layer is
  gone, so pages on other origins can't read API responses. `POST`, `PUT`,
  `PATCH`, `DELETE` and WebSocket upgrades (pod exec, port-forward) get `403`
  unless the browser reports them as same-origin: `Sec-Fetch-Site` must be
  `same-origin` or `none`, and for browsers that don't send it, `Origin` must
  match `Host` or `X-Forwarded-Host`. The UI and non-browser clients (MCP
  clients, curl and scripts, the Kubernetes API server, Git webhooks, docker
  and kaniko) are unaffected. A browser app on another origin, including a
  sibling subdomain, can no longer call the API. (#205)
- **The settings API masks secrets.** `GET /api/settings` returns
  `"configured"` in place of `notifications.webhook_url`,
  `git_token_secrets[].encrypted_token` and every `plugins[].config` value
  except keys that the loaded plugin declares non-secret (`credentials` were
  already masked). `PUT /api/settings` restores the stored value wherever it
  receives `"configured"`, so saving the masked document back no longer wipes
  anything. It never changes `credentials` or token ciphertext (use
  `POST /api/settings/credentials` and `/api/git-token-secrets`), and it
  returns `400` for a masked plugin value with nothing stored under its key,
  such as a renamed key (#207), and for an invalid endpoint or AI region
  (#253). Scripts that read secrets back from the API need another source.
- **Helm sets `RUST_LOG` only from the new `logFilter` value.** The chart
  used to hard-code `RUST_LOG=deckwatch=info,tower_http=info`. With
  `logFilter` empty, the default, deckwatch's built-in filter applies, and it
  now also keeps mcp-k8s and plugin log lines (see Changed). To keep the old
  output, set `logFilter: "deckwatch=info,tower_http=info"`. If you patched
  `RUST_LOG` onto the Deployment yourself, set `logFilter` instead. (#256)
- **Leftover plugin fields are removed after the upgrade.** When an
  application's last plugin resource is removed, deckwatch now strips the env
  vars, sidecars, init containers, volumes and annotations that the plugin
  applied to the application's workloads. The first reconcile after startup
  also cleans up workloads that earlier versions left in that state, which
  rolls out those Deployments. (#229)
- **Outbound requests have timeouts.** Git (GitOps polling, branch lookups
  and manual builds): 60 s per request. AI calls: 10 minutes. Registry probes
  and Entra JWKS: 15 s. These had none before. The Vertex AI and Bedrock
  region must now be a plain region name such as `us-east-1`. (#253)
- **S3 registry storage (object_store 0.14).** Deletes use the S3 bulk
  `DeleteObjects` API; set `AWS_DISABLE_BULK_DELETE=true` for S3-compatible
  stores that don't support it. TLS to S3 moves from OpenSSL to rustls, so an
  endpoint with a private CA needs that CA in `DECKWATCH_EXTRA_CA_FILE` (see
  Added) and a certificate that names the host in its subject alternative
  names. Object keys are unchanged, so existing buckets stay readable.
  (#240, #253)
- **Registry repository names must follow the OCI grammar:** lowercase
  alphanumeric components separated by `.`, `_`, `__` or dashes, joined by
  `/`, at most 255 characters. References must be a tag or a digest. Docker
  and kaniko already enforce this; pushes of other names are now rejected.
  (#185)
- **Plugins can't inherit `DECKWATCH_ENCRYPTION_KEY` or
  `DECKWATCH_DATABASE_URL`** through `inherit_env_keys` or
  `inherit_env_file_keys`; deckwatch logs a warning and leaves them out.
  (#207)
- **The UI needs a recent browser:** Chrome or Edge 111, Firefox 114, or
  Safari 16.4 or newer (vite 8's build target; it was Chrome 87, Firefox 78,
  Safari 14). (#237)

### Security

- **Unauthenticated file write and delete through the registry.** `/v2/*`
  accepted `..` in repository names and references (percent-encoded too), so
  anyone who could reach deckwatch could overwrite or delete files as root in
  the container, such as the SQLite database or the UI's files. Names and
  references are now validated (see Upgrade notes), and storage paths must be
  plain components. The S3 backend wasn't exploitable. Pushes and deletes on
  `/v2/*` still don't require authentication. (#185)
- **Cross-site request forgery and WebSocket hijacking.** Any web page a user
  had open could drive deckwatch through their browser: read API responses
  (CORS allowed every origin, so for example decoded Secrets through
  `POST /mcp`), make state-changing requests, and open pod exec and
  port-forward sockets. See Upgrade notes. This doesn't add authentication:
  anyone who can reach deckwatch directly can still use the API. (#205)
- **Secrets leaked from the public `GET /api/settings`:** plugin credentials
  in `plugins[].config`, notification webhook URLs and git-token ciphertext
  were returned in clear to any caller. MCP `get_plugin_config` returned
  undeclared plugin keys in clear; they are now `***` with
  `is_secret: true`. And plugin config, which anyone who can reach the API
  can edit, could pass deckwatch's encryption key to a plugin through
  `inherit_env_keys`. (#207)
- **MCP `update_deployment` ignored the namespace allowlist for
  `service_account`.** An update that set only `service_account` could point
  a Deployment in any namespace at any ServiceAccount. Only deployments that
  set `DECKWATCH_NAMESPACES` were affected. (#208)
- **Bounded label cardinality on the public `/api/frontend-metrics`.**
  Free-form labels (`route`, `path`, `kind`) take at most 128 bytes of
  visible ASCII and 512 values each; new values after that are recorded as
  `other`. `method`, the status class and web-vital names come from fixed
  sets. Before, a loop of POSTs could grow memory and the `/metrics` output
  without limit. (#208)
- **Prompt-injection fence hardened (SEC-13).** Lines that imitate the
  untrusted-data fence markers become
  `[deckwatch: removed a line that imitated the untrusted-data fence]` in the
  text that AI diagnostics and AI fixes send to a model and in MCP
  `get_pod_logs` output, so pod logs can't close the fence early. The fence
  nonce is random (128 bits; it was derived from the clock), and the prompt
  names the nonce that closes the section. (#257)
- **Stored credentials can't be redirected:** they are only sent to the
  endpoint they were saved for (see Added), and a crafted Vertex AI or
  Bedrock region can no longer redirect the access token or a signed request.
  (#253)
- **Dependency advisories.** extism 1.41 with wasmtime 49 fixes
  GHSA-mghf-wjfp-p5xm / RUSTSEC-2026-0379 (critical: an aarch64 Cranelift
  miscompile), RUSTSEC-2026-0269 (filesystem sandbox escape),
  RUSTSEC-2026-0327 (native stack buffer overflow) and about ten denial of
  service and soundness issues, all reachable because plugins are untrusted
  WASM run inside deckwatch. Also jsonwebtoken 12 (GHSA-h395-gr6q-cpjc, type
  confusion), rustls 0.23.46 (seven advisories), rustls-webpki 0.103.17 and
  webpki-roots 1.0.12, and h2, tokio, url and zstd fixes (#183). `zip`
  (three advisories, one critical; it was build-time only) and the
  unmaintained `rustls-pemfile` are no longer in the build (#236). The UI
  moves to vue 3.5.46 (a `@vue/server-renderer` XSS), and advisories in the
  frontend build and test tooling are fixed (#183, #233).
- **AES-GCM nonces come straight from the OS RNG** (rand 0.10 dropped the
  fork protection of `ThreadRng`), and a failing RNG returns an error instead
  of panicking. The stored ciphertext format is unchanged. (#216)

### Added

- **Extra trusted CAs for outbound HTTPS:** `DECKWATCH_EXTRA_CA_FILE` (a PEM
  bundle), or Helm `outbound.extraCaBundle` (`secretName` or `configMapName`,
  plus `key`). The CAs are added to the system roots for every outbound call,
  including the S3 registry backend and AWS credential providers. An
  unreadable or invalid bundle stops startup with an error that names the
  file. See [docs/TLS.md](docs/TLS.md). (#253)
- **Endpoint overrides, with credential binding.** New `endpoints` settings
  for the Anthropic API, the Vertex AI token URL and API base, Bedrock, and
  the GitHub API, web and raw bases (for GitHub Enterprise Server), in
  Settings → Advanced → Endpoints. Values must be `https://` without
  credentials, query or fragment; `http://` is accepted for localhost, or for
  any host when `DECKWATCH_ALLOW_INSECURE_ENDPOINTS=true` (Helm
  `outbound.allowInsecureEndpoints`, for development and tests). Changes are
  audit-logged. Each stored credential records the endpoint origins it was
  saved for and is never sent anywhere else, so after changing an endpoint,
  re-enter the credential. Existing credentials are bound to the default
  endpoints, and credentials read from Kubernetes Secrets only go to the
  defaults. `GET /api/settings/outbound` and Settings → Advanced show the
  defaults, the effective values, the CA bundle and where each credential may
  be sent, and the AI provider cards show which host a key was saved for.
  (#253, #254)
- **Amazon Bedrock works.** `InvokeModel` requests are signed with SigV4
  using the pod's AWS identity (IRSA, EKS Pod Identity or environment
  credentials) and are only sent to AWS hosts. Responses don't stream: the
  reply arrives as one chunk. Not yet tested against real AWS. (#253, #254)
- **Helm `logFilter`**, passed to deckwatch as `RUST_LOG`. (#256)
- **One log line per MCP tool call:** `mcp tool call`, with `tool`, `status`
  (`ok` or `error`) and `duration_ms`. Arguments and error text are never
  logged. (#256)
- **MCP `get_resource_json`** (from mcp-k8s v0.4.0) returns any resource as
  JSON. `get_resource_yaml`, which always returned JSON too, remains as a
  deprecated alias. (#252)
- **Loaded plugin binaries are identifiable.** `GET /api/plugins` adds
  `sha256` and `loaded_at`. MCP `list_plugins` and `get_plugin_config` add
  `version`, `sha256`, `wasm_size_bytes` and `loaded_at`. Settings → Plugins
  shows the SHA-256, uploads record the file's SHA-256 in the log and the
  audit log, and the OpenAPI spec now covers `/api/plugins*`. (#206)
- **`key_value` plugin config fields** (deckwatch-plugin-sdk v0.10.0), stored
  as a JSON object of strings. `POST /api/plugins/{name}/config` and MCP
  `update_plugin_config` validate them before saving anything, and the UI
  edits them as JSON text for now. MCP `update_plugin_config` also accepts an
  object for them, and `null` removes a key. Plugins that use this type need
  deckwatch 0.7.0: older releases can't parse their metadata and drop all
  their config fields and resources. (#243)
- **A plugin's `deprovision()` can ask deckwatch to delete Kubernetes
  objects** (`delete_kubernetes_resources`, deckwatch-plugin-sdk v0.10.0):
  `v1` ServiceAccounts and `batch/v1` Jobs, in the application's namespace
  only. Jobs are deleted first, with background propagation. A ServiceAccount
  that a Deployment, CronJob or Job still uses is kept, and so is `default`.
  Every request is audit-logged; refusals and failures are counted in
  `deckwatch_plugin_errors_total` (`k8s_delete_refused`, `k8s_delete_failed`)
  and reported by MCP `deprovision_resource` in
  `kubernetes_resource_deletions`. (#235)
- `User-Agent: deckwatch/<version>` on every outbound request, and outbound
  errors that include the TLS reason (unknown issuer, expired certificate,
  name mismatch). (#253)

### Changed

- **mcp-k8s and plugin log lines are kept.** The default log filter is now
  `deckwatch=info,tower_http=info,mcp_k8s=info,extism::pdk=info`; a valid
  `RUST_LOG` still replaces it. `extism::pdk` carries plugin log lines, with
  a `plugin` field. deckwatch-plugin-aws, for example, logs a few lines per
  reconcile for each deployment with AWS resources, roughly 1–2k lines an
  hour each. To keep only plugin warnings, set `logFilter` to
  `deckwatch=info,tower_http=info,mcp_k8s=info,extism::pdk=warn`. (#256)
- **Outbound HTTP clients are built once at startup**, one per purpose, with
  the timeouts listed in [docs/TLS.md](docs/TLS.md). Proxies still come from
  `HTTPS_PROXY`, `HTTP_PROXY` and `NO_PROXY`. (#253)
- **GitOps path filtering** treats a repository as GitHub-hosted only when
  its host is exactly the configured GitHub web host (it was a substring
  match on `github.com`), and no longer sends an empty `Bearer` header for
  public repositories. (#253)
- A `PUT /api/settings` without `endpoints` keeps the stored endpoints.
  (#253)
- **mcp-k8s is pinned to the v0.5.0 tag** (it followed `main`) and is built
  without its standalone server's dependencies. Tool names, parameters and
  outputs are unchanged, apart from the API versions in the upgrade notes.
  (#236, #252)
- **The UI is built with vite 8** (Rolldown and Lightning CSS), which splits
  it into smaller chunks that the page preloads. (#237)
- **web-vitals 6** measures LCP, INP and CLS slightly differently (LCP is
  final only after trusted user input), so expect a small step in the
  `deckwatch_frontend_web_vital` histograms. (#217)
- **S3 registry backend (object_store 0.14):** EKS Pod Identity is
  supported, and retries also cover HTTP 408 and 429. (#240)
- **[docs/TRACING.md](docs/TRACING.md) is rewritten** around
  deckwatch-plugin-otel, the way tracing works today. It notes that the trace
  query proxy reads `tracing` from the settings ConfigMap and works only with
  Jaeger. (#232)

### Fixed

- Saving the Settings page no longer wipes the stored Anthropic and GCP
  credentials, and saving plugin settings no longer overwrites stored secrets
  with `"configured"`. (#207)
- Uploading a plugin no longer unloads every other plugin until the next
  reload, and `deckwatch_plugins_loaded` reports the real count. (#209)
- Removing an application's last plugin resource releases what the plugin
  applied to its Deployments and CronJobs within one reconcile (10 s). Before,
  those fields stayed, and plugins kept acting on them: deckwatch-plugin-aws
  re-created an empty SQS queue from leftover `sqs.deckwatch.io/*`
  annotations. (#229)
- A plugin that declares a config field type deckwatch doesn't know no longer
  loses all its config fields and resources; the field renders as text.
  (#243)
- Settings → MCP Tuning shows each plugin's tuning fields: `GET /api/plugins`
  now returns `mcp_tuning_fields`. (#226)
- MCP `list_ipaddresses`, `get_ipaddress`, `list_servicecidrs` and
  `get_servicecidr` use GA `networking.k8s.io/v1`, which every supported
  cluster serves (they used `v1beta1`). (#252)
- The OpenAPI spec (`/api/docs`) no longer lists the removed addon
  endpoints. (#214)

### Dependencies

- **Rust:** kube 4 → 5.1 and k8s-openapi 0.28 → 0.29 (#252); mcp-k8s v0.5.0
  (#236, #252); extism 1.30 → 1.41 and jsonwebtoken 9 → 12 (#183); rand 0.8
  → 0.10, aes-gcm 0.10 → 0.11 and ed25519-dalek 2 → 3 (#216); hmac 0.12 →
  0.13 and sha2 0.10 → 0.11 (#227); object_store 0.11 → 0.14 (#240); rcgen
  0.13 → 0.14 (#238); tower-http 0.6 → 0.7 (#197); jiff 0.2 → 1.0 (#202,
  #221); metrics-exporter-prometheus 0.17 → 0.18 (#222); base64 0.22 → 0.23
  (#224). Known-answer tests pin the credential ciphertext format, license
  signature checks, webhook HMACs, OCI digests and plugin checksums across
  these upgrades, so stored credentials still decrypt (#216, #227), and the
  admission webhook's certificates have the same contents (#238).
- **Frontend:** pinia 3 → 4, with `@vue/devtools-api` 8 as a direct
  dependency, and web-vitals 4 → 6 (#217); vite 6 → 8, vitest 2 → 5 and
  happy-dom 15 → 21 (#183, #237); `@vitejs/plugin-vue` 5 → 6 (#219);
  TypeScript 5.7 → 6.0 (#231).
- **Image:** built with `rust:1.99-bookworm` (was 1.91, #186) and
  `node:26-alpine` (was 24, #231), and a pinned, checksum-verified mdBook
  (#258). The runtime base image, `gcr.io/distroless/cc-debian12`, is
  unchanged.

### Internal and CI

- CI runs `cargo test` and `cargo clippy --all-targets` (#228), the frontend
  unit tests (#213) and a type check of the test code; frontend unit tests
  fail on unmocked network requests (#218). Tests that had been failing are
  fixed (#210, #211, #212).
- The Rust toolchain is pinned to 1.99 in `rust-toolchain.toml`, and CI
  fails when the Dockerfile's `rust:` image drifts from it (#215).
- CI also runs on every push to `main`, with a read-only token (#230). A
  Docker Build job builds the release image (amd64, not pushed) on every push
  to `main`, and on PRs only when image-relevant files change; a Docs Build
  job builds the manual the way the image does (#230, #258). Rust caches are
  saved only from `main` (#258), and CI compiles without debug info (#234).
- Dependabot for cargo, npm, Docker and GitHub Actions (#184), and GitHub
  Actions updated to their current major versions (#187).
- The crate is a library plus a thin binary, so tests in `tests/` can call
  real code; `tests/outbound_tls.rs` checks every outbound call against local
  TLS mocks. (#253)
- Stale references to the removed addon system are gone from `CLAUDE.md` and
  the docs. (#214)
- **Releases:** this changelog. The release workflow refuses a tag that
  doesn't match the versions and a dated section here, before it builds
  anything, and then creates the GitHub Release from that section. CI's
  Release Check job runs the same check on every PR. (#264)

## Before 0.7.0

Releases up to 0.6.7 have no changelog. See their
[tags](https://github.com/alexconrey/deckwatch/tags) and, for some of them,
[GitHub Releases](https://github.com/alexconrey/deckwatch/releases).

[Unreleased]: https://github.com/alexconrey/deckwatch/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/alexconrey/deckwatch/compare/v0.6.7...v0.7.0
