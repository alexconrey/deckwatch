#!/usr/bin/env bash
# Usage: .github/scripts/release-on-main.sh [<commit>]   (default: HEAD)
#
# Refuses (exit 1) to release a commit that isn't on main. It fetches main
# from origin and accepts <commit> only if it is main's tip or one of its
# ancestors, so a tag pushed on any other branch publishes nothing. The
# Release workflow (publish.yml) runs it in its verify job, next to
# release-notes.sh, with the tagged commit; every other job needs that one.
#
# It works in the checkout it lives in, whatever the current directory. A
# shallow clone is deepened first, so the ancestry check sees all of main.
# Errors go to stderr, as GitHub Actions annotations when it runs in a
# workflow. Tests: .github/scripts/release-notes-test.sh.
set -euo pipefail

if [ $# -gt 1 ]; then
  echo "usage: $0 [<commit>]" >&2
  exit 2
fi
commit=${1:-HEAD}

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

error() {
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::error::$1" >&2
  else
    echo "error: $1" >&2
  fi
}

# Annotated tags name a tag object; the check needs the commit it points to.
if ! sha=$(git rev-parse --verify --quiet "${commit}^{commit}"); then
  error "'$commit' is not a commit"
  exit 1
fi

fetch_options=(--quiet --no-tags)
if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
  fetch_options+=(--unshallow)
fi
if ! git fetch "${fetch_options[@]}" origin "+refs/heads/main:refs/remotes/origin/main"; then
  error "could not fetch main from origin, so the commit can't be checked"
  exit 1
fi

# Exit status 1 means "not an ancestor"; anything else is an error. Both refuse.
if ! git merge-base --is-ancestor "$sha" refs/remotes/origin/main; then
  error "commit $sha is not on main: tag a commit on main, after its PR has merged"
  exit 1
fi
echo "commit $sha is on main"
