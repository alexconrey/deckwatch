#!/usr/bin/env bash
# Run the Playwright e2e suite (functional and visual specs) inside the
# official Playwright Docker image, the same environment as the CI `e2e` job.
# The visual baselines are only valid in this image. See docs/TESTING.md.
#
#   scripts/e2e-docker.sh                               # whole suite
#   scripts/e2e-docker.sh --project=visual              # visual specs only
#   scripts/e2e-docker.sh e2e/visual/settings.spec.ts   # one file
#   scripts/e2e-docker.sh --update-snapshots            # rewrite baselines
#
# Arguments are passed to `playwright test`. The image tag is derived from
# the @playwright/test version in pnpm-lock.yaml, so it always matches the
# installed library. linux/amd64 is forced (CI runners are amd64; on Apple
# silicon Docker runs it under Rosetta).
set -euo pipefail

cd "$(dirname "$0")/.."

version=$(sed -nE "s/^  '@playwright\/test@([0-9][^']*)':\$/\1/p" pnpm-lock.yaml | head -n 1)
if [ -z "$version" ]; then
  echo "could not find @playwright/test in pnpm-lock.yaml" >&2
  exit 1
fi
image="mcr.microsoft.com/playwright:v${version}-noble"

# node_modules must hold Linux binaries, so it lives in a Docker volume that
# shadows the host's node_modules. One volume per checkout (worktrees differ).
key=$(printf '%s' "$PWD" | shasum | cut -c1-12)

tty=()
[ -t 1 ] && tty=(-t)

exec docker run --rm --init ${tty[@]+"${tty[@]}"} \
  --platform linux/amd64 \
  --ipc=host \
  -v "$PWD:/work" \
  -v "deckwatch-e2e-node-modules-$key:/work/node_modules" \
  -v "deckwatch-e2e-pnpm-store:/pnpm-store" \
  -w /work \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  -e E2E_ALL_BROWSERS -e PLAYWRIGHT_HTML_OPEN=never \
  "$image" \
  bash -euo pipefail -c '
    restore_owner() {
      chown -R "$HOST_UID:$HOST_GID" dist test-results playwright-report e2e/__screenshots__ 2>/dev/null || true
    }
    trap restore_owner EXIT
    npm install --global --silent --no-fund --no-audit pnpm@11 >/dev/null
    pnpm config set store-dir /pnpm-store >/dev/null
    pnpm config set package-import-method copy >/dev/null
    pnpm install --frozen-lockfile --reporter=silent
    pnpm exec playwright test "$@"
  ' e2e "$@"
