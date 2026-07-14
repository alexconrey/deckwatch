# AI Diagnostics

The **Diagnose with AI** feature lets an operator hand a slice of pod logs to
an LLM agent (Claude or Codex) and get back a diagnosis. The agent runs
inside the target namespace as a short-lived Kubernetes Job so that all
credentials, network paths, and audit trails stay within the cluster.

## When the button appears

The button is rendered by `frontend/src/components/common/DiagnoseButton.vue`
inside the log viewer. It becomes visible when *either* condition holds:

1. The pod phase is one of `CrashLoopBackOff`, `Error`, `Failed`,
   `ImagePullBackOff`, `ErrImagePull`, or `CreateContainerError`.
2. The currently-loaded log text contains a whole-word match for `error`
   or `fatal` (case-insensitive).

If neither condition holds, the button stays hidden — this is intentional
to avoid nudging the operator to burn LLM tokens on healthy pods.

## Agent selection

- On first click, a dialog offers **Claude** and **Codex**. The choice is
  saved to `sessionStorage` under the key `deckwatch-ai-agent` and reused
  for the remainder of the browser session.
- A small `Using <Agent>` chip is shown next to the button once a choice is
  saved, with a `Change` link that clears the preference and re-prompts.

## What happens on submit

1. `POST /api/namespaces/{ns}/diagnostics` is called with the pod name,
   container (optional), the log text, and the chosen agent.
2. The backend (`src/handlers/diagnostics.rs`):
   - Truncates logs to the last 256 KiB (LLMs choke on huge blobs, and
     the failure signal is almost always at the tail).
   - Creates a **ConfigMap** in the same namespace holding the logs and
     the prompt. This is cheaper and safer than stuffing multi-KiB
     payloads into env vars, and it survives Job pod restarts if the
     platform ever adds retries.
   - Creates a **Job** in the same namespace with:
     - `restartPolicy: Never`, `backoffLimit: 0` (no accidental
       re-billing on failure)
     - `ttlSecondsAfterFinished: 3600` (auto-cleanup after 1h)
     - `activeDeadlineSeconds: 600` (10 min hard cap)
     - The agent image mounted with the log ConfigMap at `/diag/pod.log`
     - The API key injected from a namespaced Secret (see below)
   - Returns `{ job_name, status, agent }`.
3. The frontend polls `GET /api/namespaces/{ns}/diagnostics/{job}` every
   3 s until the Job reaches a terminal state (`succeeded` or `failed`),
   then fetches `GET /api/namespaces/{ns}/diagnostics/{job}/result` which
   returns the log output of the newest Job pod — i.e., what the agent
   wrote to stdout.

## Agent configuration

Configured in `helm/deckwatch/values.yaml`:

```yaml
diagnostics:
  enabled: true
  claude:
    image: ghcr.io/anthropics/claude-code:latest
    apiKeySecret: deckwatch-anthropic-api-key
  codex:
    image: ghcr.io/openai/codex:latest
    apiKeySecret: deckwatch-openai-api-key
```

These values are surfaced to the backend as environment variables on the
deckwatch deployment:

| Env var                             | Purpose                                            |
|-------------------------------------|----------------------------------------------------|
| `DECKWATCH_DIAG_CLAUDE_IMAGE`       | Container image for Claude agent runs              |
| `DECKWATCH_DIAG_CLAUDE_SECRET`      | Secret name (in target ns) holding Anthropic key   |
| `DECKWATCH_DIAG_CODEX_IMAGE`        | Container image for Codex agent runs               |
| `DECKWATCH_DIAG_CODEX_SECRET`       | Secret name (in target ns) holding OpenAI key      |

## API key Secrets

API keys are **never** baked into images or read from files on disk. They
are read from `Secret` objects that you provision, one per namespace where
diagnostics will run:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: deckwatch-anthropic-api-key
  namespace: my-app
type: Opaque
stringData:
  api-key: sk-ant-...
```

The Secret's key **must** be `api-key`. The agent container receives it as
`ANTHROPIC_API_KEY` (Claude) or `OPENAI_API_KEY` (Codex).

Kubernetes does not allow cross-namespace Secret references — you must
create one Secret per target namespace, or scope deckwatch to only
namespaces that have the Secret provisioned.

## RBAC requirements

Added to `helm/deckwatch/templates/clusterrole.yaml`:

```yaml
- apiGroups: [""]
  resources: ["configmaps"]
  verbs: ["get", "list", "create", "delete"]
```

Deckwatch already had `create` on `batch/jobs`, so no change was needed
for the Job side. The `secrets` verb stays `get`-only — the diagnostic Job
consumes the Secret via `secretKeyRef`, which resolves inside the
kubelet, so the deckwatch process itself never reads the plaintext key.

## Architecture

```
+-----------------------+           +----------------------+
|  Browser (Vue)        |           |  Deckwatch API       |
|  LogViewer.vue        | -- HTTP-> |  /api/.../diagnostics|
|  DiagnoseButton.vue   |           |  (Axum, kube-rs)     |
+-----------------------+           +----------+-----------+
                                               |
                                               | kube-rs
                                               v
                              +----------------+-----------------+
                              |  Target namespace                |
                              |                                  |
                              |  ConfigMap (logs + prompt)       |
                              |          |                       |
                              |          v                       |
                              |  Job -> Pod                      |
                              |    image: claude-code / codex    |
                              |    env: API key from Secret      |
                              |    stdin: prompt + logs          |
                              |    stdout: diagnosis <-----------+
                              |                                  |     poll status
                              +----------------------------------+ <---------------- Deckwatch
                                                                       fetch result
```

Data flow:

1. `logs` (string) → truncated → written into a ConfigMap `data["pod.log"]`.
2. Job pod mounts ConfigMap at `/diag/pod.log`.
3. Entry script `cat`s the file, concatenates it after the prompt, pipes
   into the agent CLI on `stdin`.
4. Agent writes diagnosis to `stdout`. Kubernetes captures stdout as pod
   logs, which deckwatch reads via `kube::api::Api::logs`.

## Follow-ups / limitations

- **Cost visibility**: no per-run token accounting yet. Consider surfacing
  the agent's usage report (if the CLI prints one) or adding a per-user
  rate limit.
- **Multi-container pods**: the frontend already passes `container` in the
  request but the current agent prompt does not call it out explicitly.
  Include it in the prompt template if you find agents guessing wrong
  containers.
- **Result streaming**: results are fetched only after the Job reaches a
  terminal state. Streaming stdout during the run (via SSE, like the log
  viewer) would improve UX for long diagnoses.
- **Cleanup on failure**: the log ConfigMap is best-effort deleted if
  Job creation fails, but successful runs leave the ConfigMap around
  until the Job TTL fires and garbage-collects the owned resources.
  If Jobs are cleaned by TTL controller, ensure ConfigMaps have
  ownerReferences to the Job so they're collected too (currently they
  don't — follow-up).
- **Prompt injection**: pod logs are user-controlled data being fed to an
  LLM. Agents may execute embedded instructions. Mitigate by running the
  agent CLI in a non-interactive mode with no shell tools enabled if the
  chosen CLI supports it (Claude Code's `--dangerously-skip-permissions`
  is used here purely to avoid interactive TTY prompts inside a
  batch Job; consider a more restrictive flag if/when available).
