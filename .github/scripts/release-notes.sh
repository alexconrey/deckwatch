#!/usr/bin/env bash
# Usage: .github/scripts/release-notes.sh <tag>        (for example v0.7.0)
#
# Decides whether <tag> may be released and, if it may, prints its release
# notes. The Release workflow (publish.yml) runs it before it builds or pushes
# anything, so a tag this script refuses publishes nothing. Run it locally to
# preview the notes or the refusal.
#
# The tag is refused (exit 1, with every problem reported) unless:
#   - it is vX.Y.Z;
#   - X.Y.Z is the version in Cargo.toml ([package] version),
#     helm/deckwatch/Chart.yaml (version and appVersion) and
#     frontend/package.json (version);
#   - CHANGELOG.md has exactly one "## [X.Y.Z] - YYYY-MM-DD" section: dated,
#     so a section still marked "Unreleased" can't be released, and with at
#     least one entry.
#
# On success it prints that section without its heading. Errors go to stderr,
# as GitHub Actions annotations when it runs in a workflow. It reads the
# checkout it lives in, whatever the current directory. Needs bash, awk, grep,
# sed and jq. Tests: .github/scripts/release-notes-test.sh.
set -euo pipefail

if [ $# -ne 1 ]; then
  echo "usage: $0 <tag>   (for example v0.7.0)" >&2
  exit 2
fi
tag=$1

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

failed=0
# error <file, or -> <message> [<line>]: report a problem and keep checking.
error() {
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    local where=""
    [ "$1" != "-" ] && where=" file=$1${3:+,line=$3}"
    echo "::error${where}::$2" >&2
  elif [ "$1" = "-" ]; then
    echo "error: $2" >&2
  else
    echo "error: $1${3:+:$3}: $2" >&2
  fi
  failed=1
}

semver='^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
if [[ ! $tag =~ $semver ]]; then
  error - "tag '$tag' is not of the form vX.Y.Z"
  exit 1
fi
version=${tag#v}
version_re=${version//./\\.}

cargo=Cargo.toml
chart=helm/deckwatch/Chart.yaml
package=frontend/package.json
changelog=CHANGELOG.md

for f in "$cargo" "$chart" "$package" "$changelog"; do
  [ -f "$f" ] || error "$f" "file not found"
done
[ "$failed" = 0 ] || exit 1

# check <file> <field> <value found>
check() {
  if [ -z "$3" ]; then
    error "$1" "no $2 found"
  elif [ "$3" != "$version" ]; then
    error "$1" "$2 is $3, but the tag is $tag"
  fi
}

# The version key of the [package] table, not of a dependency or other table.
check "$cargo" "[package] version" "$(awk '
  { sub(/\r$/, "") }
  /^\[/ { in_package = ($0 ~ /^\[package\][ \t]*(#.*)?$/); next }
  in_package && /^version[ \t]*=/ {
    sub(/^version[ \t]*=[ \t]*"/, ""); sub(/".*$/, ""); print; exit
  }' "$cargo")"

# A top-level scalar of Chart.yaml, without quotes or a trailing comment.
chart_value() {
  awk -v key="$1" -v q="'" '
    { sub(/\r$/, "") }
    index($0, key ":") == 1 {
      v = substr($0, length(key) + 2)
      sub(/[ \t]+#.*$/, "", v)
      gsub(/^[ \t]+|[ \t]+$/, "", v)
      if (v ~ /^".*"$/ || v ~ ("^" q ".*" q "$")) v = substr(v, 2, length(v) - 2)
      print v
      exit
    }' "$chart"
}
check "$chart" "version" "$(chart_value version)"
check "$chart" "appVersion" "$(chart_value appVersion)"

if command -v jq > /dev/null; then
  check "$package" "version" "$(jq -r '.version // empty' "$package" 2> /dev/null || true)"
else
  error "$package" "jq is needed to read the version"
fi

# Headings for this version: "## [X.Y.Z]", then a space or the end of the line.
headings=$(grep -n -E "^## \[${version_re}\]([[:space:]]|$)" "$changelog" || true)
want="## [$version] - YYYY-MM-DD"
if [ -z "$headings" ]; then
  error "$changelog" "no '$want' section"
elif [ "$(grep -c . <<< "$headings")" -gt 1 ]; then
  error "$changelog" "more than one '## [$version]' section (lines $(cut -d: -f1 <<< "$headings" | paste -sd, -))"
else
  line=${headings%%:*}
  heading=$(sed -E 's/[[:space:]]+$//' <<< "${headings#*:}")
  date_re='[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])'
  if ! grep -q -E "^## \[${version_re}\] - ${date_re}$" <<< "$heading"; then
    if grep -q -i 'unreleased' <<< "$heading"; then
      error "$changelog" "'$heading' is not dated: the release PR replaces 'Unreleased' with the release date ('$want')" "$line"
    else
      error "$changelog" "'$heading' must be '$want'" "$line"
    fi
  fi

  # The section body: after the heading, up to the next "#" or "##" heading
  # or the link references at the end, without surrounding blank lines.
  notes=$(awk -v start="$line" '
    NR <= start { next }
    { sub(/\r$/, "") }
    /^##?[ \t]/ || /^\[[^]]+\]:[ \t]/ { exit }
    { lines[++n] = $0; if ($0 ~ /[^ \t]/) { if (!first) first = n; last = n } }
    END { for (i = first; first && i <= last; i++) print lines[i] }
  ' "$changelog")
  # An entry is any line that is neither blank nor a heading.
  if ! grep -q -E '^[[:space:]]*[^#[:space:]]' <<< "$notes"; then
    error "$changelog" "the '## [$version]' section has no entries" "$line"
  fi
fi

[ "$failed" = 0 ] || exit 1
printf '%s\n' "$notes"
