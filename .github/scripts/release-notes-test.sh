#!/usr/bin/env bash
# Tests for release-notes.sh and release-on-main.sh. CI runs them (ci.yml,
# Release Check); locally:
#   bash .github/scripts/release-notes-test.sh
#
# Each case builds a small repository in a temporary directory, with a copy of
# the script, and checks the exit status, the error messages, and the notes
# (a refused tag must print no notes at all). The release-on-main.sh cases use
# git repositories: an origin, and clones of it.
set -euo pipefail

# The cases expect the local "error: <file>: <message>" format, also in CI;
# the annotation format has its own case at the end.
unset GITHUB_ACTIONS

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
passed=0
failures=0

changelog_ok='# Changelog

Header text that is not part of any release.

## [Unreleased]

### Added
- Something that is not released yet.

## [0.7.0] - 2026-11-24

Summary of the release.

### Fixed
- A fix. (#1)

## Before 0.7.0

Releases before 0.7.0 have no changelog.

[Unreleased]: https://example.com/compare/v0.7.0...HEAD
[0.7.0]: https://example.com/compare/v0.6.7...v0.7.0'

notes_ok='Summary of the release.

### Fixed
- A fix. (#1)'

# repo <name> [cargo] [chart version] [chart appVersion] [package.json]
# Creates a repository that release-notes.sh accepts for v0.7.0 unless a
# version argument says otherwise, and prints its path.
repo() {
  local dir="$work/$1"
  mkdir -p "$dir/.github/scripts" "$dir/helm/deckwatch" "$dir/frontend"
  cp "$here/release-notes.sh" "$dir/.github/scripts/"
  # Other tables' version keys, before and after [package], must be ignored.
  cat > "$dir/Cargo.toml" << EOF
[workspace.metadata.decoy]
version = "9.9.9"

[package]
name = "deckwatch"
version = "${2-0.7.0}"
edition = "2021"

[dependencies]
kube = { version = "5", features = ["ws"] }

[dependencies.decoy]
version = "8.8.8"
EOF
  cat > "$dir/helm/deckwatch/Chart.yaml" << EOF
apiVersion: v2
name: deckwatch
type: application
version: ${3-0.7.0}
appVersion: "${4-0.7.0}"
kubeVersion: ">=1.33.0-0"
EOF
  cat > "$dir/frontend/package.json" << EOF
{
  "name": "deckwatch-frontend",
  "version": "${5-0.7.0}",
  "engines": { "node": ">=24" },
  "dependencies": { "pinia": "^4.1.0" }
}
EOF
  printf '%s\n' "$changelog_ok" > "$dir/CHANGELOG.md"
  printf '%s\n' "$dir"
}

# changelog <dir> <content>: replace the repository's CHANGELOG.md.
changelog() { printf '%s\n' "$2" > "$1/CHANGELOG.md"; }

# check <description> <dir> <tag> <exit status> <stdout, or - for any> [<text stderr must contain>...]
check() {
  local desc=$1 dir=$2 tag=$3 want_status=$4 want_out=$5
  shift 5
  local out err status=0 problems=""
  out=$(bash "$dir/.github/scripts/release-notes.sh" "$tag" 2> "$work/stderr") || status=$?
  err=$(cat "$work/stderr")
  [ "$status" = "$want_status" ] || problems="$problems
    exit status $status, want $want_status"
  if [ "$want_out" != "-" ] && [ "$out" != "$want_out" ]; then
    problems="$problems
    stdout:
$out
    want:
$want_out"
  fi
  local text
  for text in "$@"; do
    case "$err" in
      *"$text"*) ;;
      *) problems="$problems
    stderr lacks: $text" ;;
    esac
  done
  if [ -z "$problems" ]; then
    passed=$((passed + 1))
    echo "ok   $desc"
  else
    failures=$((failures + 1))
    echo "FAIL $desc$problems"
    echo "    stderr: $err"
  fi
}

# --- Accepted -------------------------------------------------------------

check "a matching tag passes and prints only its own section" \
  "$(repo ok)" v0.7.0 0 "$notes_ok"

dir=$(repo quoting)
cat > "$dir/helm/deckwatch/Chart.yaml" << 'EOF'
apiVersion: v2
name: deckwatch
version: "0.7.0" # quoted, with a comment
appVersion: '0.7.0'
EOF
check "quoted Chart.yaml values and trailing comments are read" "$dir" v0.7.0 0 "$notes_ok"

dir=$(repo last-section)
changelog "$dir" '# Changelog

## [Unreleased]

## [0.7.0] - 2026-11-24
- The only entry.


[Unreleased]: https://example.com/compare/v0.7.0...HEAD
[0.7.0]: https://example.com/releases/tag/v0.7.0'
check "the last section ends at the link references" "$dir" v0.7.0 0 "- The only entry."

dir=$(repo crlf)
printf '%s\n' "$changelog_ok" | awk '{ printf "%s\r\n", $0 }' > "$dir/CHANGELOG.md"
awk '{ printf "%s\r\n", $0 }' "$dir/helm/deckwatch/Chart.yaml" > "$dir/chart.tmp"
mv "$dir/chart.tmp" "$dir/helm/deckwatch/Chart.yaml"
check "CRLF line endings are accepted, and stripped from the notes" "$dir" v0.7.0 0 "$notes_ok"

# --- Refused: the tag ------------------------------------------------------

i=0
for bad in 0.7.0 v0.7 v0.7.0-rc.1 v01.7.0 release-0.7.0 refs/tags/v0.7.0 "v0.7.0 "; do
  i=$((i + 1))
  check "tag '$bad' is refused" "$(repo "tag-$i")" "$bad" 1 "" \
    "tag '$bad' is not of the form vX.Y.Z"
done

check "a tag older than the versions is refused" "$(repo older)" v0.6.9 1 "" \
  "Cargo.toml: [package] version is 0.7.0, but the tag is v0.6.9"

# --- Refused: one version field disagrees ----------------------------------

check "Cargo.toml mismatch" "$(repo cargo 0.6.7)" v0.7.0 1 "" \
  "error: Cargo.toml: [package] version is 0.6.7, but the tag is v0.7.0"

check "Chart.yaml version mismatch" "$(repo chart-version 0.7.0 0.6.7)" v0.7.0 1 "" \
  "error: helm/deckwatch/Chart.yaml: version is 0.6.7, but the tag is v0.7.0"

check "Chart.yaml appVersion mismatch" "$(repo chart-app 0.7.0 0.7.0 0.6.7)" v0.7.0 1 "" \
  "error: helm/deckwatch/Chart.yaml: appVersion is 0.6.7, but the tag is v0.7.0"

check "frontend/package.json mismatch" "$(repo package 0.7.0 0.7.0 0.7.0 0.6.7)" v0.7.0 1 "" \
  "error: frontend/package.json: version is 0.6.7, but the tag is v0.7.0"

check "a missing version field is reported" "$(repo no-app 0.7.0 0.7.0 '')" v0.7.0 1 "" \
  "error: helm/deckwatch/Chart.yaml: no appVersion found"

dir=$(repo no-package-table)
printf '[workspace]\nmembers = []\nversion = "0.7.0"\n' > "$dir/Cargo.toml"
check "a version outside [package] doesn't count" "$dir" v0.7.0 1 "" \
  "error: Cargo.toml: no [package] version found"

check "every mismatch is reported in one run" "$(repo all 0.6.1 0.6.2 0.6.3 0.6.4)" v0.7.0 1 "" \
  "[package] version is 0.6.1" "version is 0.6.2" "appVersion is 0.6.3" \
  "frontend/package.json: version is 0.6.4"

dir=$(repo missing-file)
rm "$dir/frontend/package.json"
check "a missing file is reported" "$dir" v0.7.0 1 "" \
  "error: frontend/package.json: file not found"

# --- Refused: CHANGELOG.md -------------------------------------------------

# Near misses must not count as the 0.7.0 section: a longer version, and one
# that an unescaped "." in the version pattern would match.
dir=$(repo no-section)
changelog "$dir" '# Changelog

## [Unreleased]
- Pending.

## [0.7.01] - 2026-11-24
- A different version.

## [017.0] - 2026-11-23
- A typo.

## [0.6.7] - 2026-11-06
- Old.'
check "a missing section is refused" "$dir" v0.7.0 1 "" \
  "error: CHANGELOG.md: no '## [0.7.0] - YYYY-MM-DD' section"

dir=$(repo unreleased)
changelog "$dir" '# Changelog

## [0.7.0] - Unreleased

### Added
- Not released yet.'
check "an 'Unreleased' section is refused" "$dir" v0.7.0 1 "" \
  "error: CHANGELOG.md:3: '## [0.7.0] - Unreleased' is not dated"

dir=$(repo undated)
changelog "$dir" '# Changelog

## [0.7.0]
- An entry.'
check "an undated heading is refused" "$dir" v0.7.0 1 "" \
  "error: CHANGELOG.md:3: '## [0.7.0]' must be '## [0.7.0] - YYYY-MM-DD'"

i=0
for heading in "## [0.7.0] - 2026-13-01" "## [0.7.0] - 24/11/2026" "## [0.7.0] 2026-11-24" "## [0.7.0] - 2026-11-24 (yanked)"; do
  i=$((i + 1))
  dir=$(repo "bad-date-$i")
  changelog "$dir" "# Changelog

$heading
- An entry."
  check "heading '$heading' is refused" "$dir" v0.7.0 1 "" "'$heading' must be"
done

dir=$(repo empty)
changelog "$dir" '# Changelog

## [Unreleased]

## [0.7.0] - 2026-11-24

## [0.6.7] - 2026-11-06
- Old.'
check "an empty section is refused" "$dir" v0.7.0 1 "" \
  "error: CHANGELOG.md:5: the '## [0.7.0]' section has no entries"

dir=$(repo headings-only)
changelog "$dir" '# Changelog

## [0.7.0] - 2026-11-24

### Added

### Fixed

[0.7.0]: https://example.com/releases/tag/v0.7.0'
check "a section with only headings is refused" "$dir" v0.7.0 1 "" \
  "the '## [0.7.0]' section has no entries"

dir=$(repo duplicate)
changelog "$dir" '# Changelog

## [0.7.0] - 2026-11-24
- One.

## [0.7.0] - 2026-11-25
- Two.'
check "two sections for the version are refused" "$dir" v0.7.0 1 "" \
  "more than one '## [0.7.0]' section (lines 3,6)"

# --- Output format ---------------------------------------------------------

dir=$(repo annotations 0.6.7)
changelog "$dir" '## [0.7.0] - Unreleased
- Pending.'
out=$(GITHUB_ACTIONS=true bash "$dir/.github/scripts/release-notes.sh" v0.7.0 2>&1 || true)
case "$out" in
  *"::error file=Cargo.toml::[package] version is 0.6.7, but the tag is v0.7.0"*"::error file=CHANGELOG.md,line=1::'## [0.7.0] - Unreleased' is not dated"*)
    passed=$((passed + 1))
    echo "ok   GitHub Actions annotations name the file and line"
    ;;
  *)
    failures=$((failures + 1))
    echo "FAIL GitHub Actions annotations name the file and line"
    echo "    output: $out"
    ;;
esac

status=0
bash "$here/release-notes.sh" > /dev/null 2>&1 || status=$?
if [ "$status" = 2 ]; then
  passed=$((passed + 1))
  echo "ok   no tag is a usage error (exit 2)"
else
  failures=$((failures + 1))
  echo "FAIL no tag is a usage error (exit 2): exit status $status"
fi

# --- release-on-main.sh: the tagged commit must be on main ------------------

# git with an identity, for building the test repositories.
g() {
  git -c user.name=test -c user.email=test@example.com -c init.defaultBranch=main \
    -c advice.detachedHead=false "$@"
}

# origin: main has two commits; branch "side" has one more that main lacks.
# v0.0.1 (annotated) tags main's tip, v0.0.2 (annotated) the side commit.
origin="$work/origin.git"
seed="$work/seed"
g init --quiet --bare "$origin"
g init --quiet "$seed"
g -C "$seed" commit --quiet --allow-empty -m one
g -C "$seed" commit --quiet --allow-empty -m two
old=$(git -C "$seed" rev-parse HEAD~1)
tip=$(git -C "$seed" rev-parse HEAD)
g -C "$seed" switch --quiet -c side
g -C "$seed" commit --quiet --allow-empty -m unmerged
side=$(git -C "$seed" rev-parse HEAD)
g -C "$seed" tag -a -m "on main" v0.0.1 "$tip"
g -C "$seed" tag -a -m "on side" v0.0.2 "$side"
g -C "$seed" push --quiet "$origin" main side v0.0.1 v0.0.2

# clone <name> [git clone options]: a clone of origin holding a copy of the
# script; prints its path.
clone() {
  local dir="$work/$1"
  shift
  g clone --quiet "$@" "file://$origin" "$dir" || return
  mkdir -p "$dir/.github/scripts"
  cp "$here/release-on-main.sh" "$dir/.github/scripts/"
  printf '%s\n' "$dir"
}

# check_on_main <description> <dir> <commit> <exit status> [<text the output must contain>...]
check_on_main() {
  local desc=$1 dir=$2 commit=$3 want_status=$4
  shift 4
  local out status=0 problems="" text
  out=$(bash "$dir/.github/scripts/release-on-main.sh" "$commit" 2>&1) || status=$?
  [ "$status" = "$want_status" ] || problems="$problems
    exit status $status, want $want_status"
  for text in "$@"; do
    case "$out" in
      *"$text"*) ;;
      *) problems="$problems
    output lacks: $text" ;;
    esac
  done
  if [ -z "$problems" ]; then
    passed=$((passed + 1))
    echo "ok   $desc"
  else
    failures=$((failures + 1))
    echo "FAIL $desc$problems"
    echo "    output: $out"
  fi
}

dir=$(clone main-ok)
check_on_main "main's tip is accepted" "$dir" "$tip" 0 "commit $tip is on main"
check_on_main "an older commit on main is accepted" "$dir" "$old" 0 "commit $old is on main"
check_on_main "an annotated tag on main is accepted" "$dir" v0.0.1 0 "commit $tip is on main"
check_on_main "a commit only on another branch is refused" "$dir" "$side" 1 \
  "error: commit $side is not on main"
check_on_main "an annotated tag on another branch is refused" "$dir" v0.0.2 1 \
  "error: commit $side is not on main"
check_on_main "something that isn't a commit is refused" "$dir" no-such-ref 1 \
  "error: 'no-such-ref' is not a commit"

dir=$(clone main-ahead)
g -C "$dir" commit --quiet --allow-empty -m "not pushed"
check_on_main "a commit on the local main but not origin's is refused" "$dir" HEAD 1 \
  "is not on main"

dir=$(clone main-shallow --depth 1 --branch main)
check_on_main "a shallow clone: main's tip is accepted" "$dir" HEAD 0 \
  "commit $tip is on main"
if [ "$(git -C "$dir" rev-parse --is-shallow-repository)" = false ]; then
  passed=$((passed + 1))
  echo "ok   a shallow clone is deepened before the check"
else
  failures=$((failures + 1))
  echo "FAIL a shallow clone is deepened before the check: it is still shallow"
fi

dir=$(clone main-no-origin)
git -C "$dir" remote set-url origin "file://$work/missing.git"
check_on_main "a main that can't be fetched is refused" "$dir" HEAD 1 \
  "error: could not fetch main from origin"

dir=$(clone main-annotations)
out=$(GITHUB_ACTIONS=true bash "$dir/.github/scripts/release-on-main.sh" "$side" 2>&1 || true)
case "$out" in
  *"::error::commit $side is not on main"*)
    passed=$((passed + 1))
    echo "ok   release-on-main.sh reports a GitHub Actions annotation"
    ;;
  *)
    failures=$((failures + 1))
    echo "FAIL release-on-main.sh reports a GitHub Actions annotation"
    echo "    output: $out"
    ;;
esac

status=0
bash "$here/release-on-main.sh" one two > /dev/null 2>&1 || status=$?
if [ "$status" = 2 ]; then
  passed=$((passed + 1))
  echo "ok   release-on-main.sh with two arguments is a usage error (exit 2)"
else
  failures=$((failures + 1))
  echo "FAIL release-on-main.sh with two arguments is a usage error (exit 2): exit status $status"
fi

echo
echo "$passed passed, $failures failed"
[ "$failures" = 0 ]
