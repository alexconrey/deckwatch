#!/usr/bin/env bash
# Build the deckwatch mdBook manual into docs/book/book/.
#
# The canonical documentation lives in docs/*.md. The book source directory
# (docs/book/src) contains only the curated SUMMARY.md, landing README, and
# api.md — the rest of the pages are pulled in from docs/*.md via SUMMARY
# entries. mdBook needs those .md files reachable under its `src` dir, so
# this script stages symlinks (or copies, in the Docker build) before
# invoking `mdbook build`.
#
# Usage:
#   scripts/build-docs.sh              # symlink-and-build (default, dev mode)
#   scripts/build-docs.sh --copy       # copy sources into src/ (for Docker)
#
# Output: docs/book/book/index.html and the rest of the rendered site.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
book_dir="${repo_root}/docs/book"
src_dir="${book_dir}/src"
docs_dir="${repo_root}/docs"

mode="symlink"
if [[ "${1:-}" == "--copy" ]]; then
  mode="copy"
fi

if ! command -v mdbook >/dev/null 2>&1; then
  echo "error: mdbook not found on PATH" >&2
  echo "install with: cargo install mdbook --version ^0.4" >&2
  exit 1
fi

# The SUMMARY.md references docs via ../../<NAME>.md. mdBook resolves those
# relative to `src/`, so the physical files need to be at docs/<NAME>.md
# from the perspective of src/ — which is exactly where they already are.
# But mdBook >=0.4.36 refuses paths that escape `src/` unless the file is
# reachable through a symlink INSIDE src/. So we create a `src/docs`
# passthrough that points at the real docs/ tree, then normalise SUMMARY.md
# links to `./docs/<NAME>.md` at build time via sed.
#
# We stage into a temporary SUMMARY so the canonical one on disk stays as
# authored (with the ../../ style that reads naturally next to book.toml).

staged_docs="${src_dir}/docs"
if [[ -e "${staged_docs}" && ! -L "${staged_docs}" ]]; then
  rm -rf "${staged_docs}"
fi

case "${mode}" in
  symlink)
    ln -sfn "${docs_dir}" "${staged_docs}"
    ;;
  copy)
    rm -rf "${staged_docs}"
    mkdir -p "${staged_docs}"
    # Copy only *.md so we don't drag the whole book/ tree back into itself.
    find "${docs_dir}" -maxdepth 1 -type f -name '*.md' -exec cp {} "${staged_docs}/" \;
    ;;
esac

# Rewrite ../../<NAME>.md → ./docs/<NAME>.md in a working copy of SUMMARY.md
# so the on-disk file stays canonical. `mdbook build` reads whatever is at
# src/SUMMARY.md when it runs, so we swap-then-restore.
summary="${src_dir}/SUMMARY.md"
summary_bak="${summary}.orig"
trap 'if [[ -f "${summary_bak}" ]]; then mv "${summary_bak}" "${summary}"; fi' EXIT

cp "${summary}" "${summary_bak}"
sed -E -i.tmp 's#\]\(\.\./\.\./([^)]+)\)#](./docs/\1)#g' "${summary}"
rm -f "${summary}.tmp"

cd "${book_dir}"
mdbook build

echo "book built at: ${book_dir}/book"
