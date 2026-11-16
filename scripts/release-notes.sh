#!/usr/bin/env bash
# Release notes from conventional commits.
#
#   scripts/release-notes.sh [<tag>]
#
# Lists the commits between the previous tag and <tag> (default: HEAD),
# grouped by conventional-commit type. Merge commits and `[skip ci]`
# bookkeeping are left out. Output is Markdown on stdout, which the release
# workflow attaches to the draft release.
set -euo pipefail

to="${1:-HEAD}"
if from=$(git describe --tags --abbrev=0 "${to}^" 2>/dev/null); then
  range="${from}..${to}"
  echo "Changes since ${from}:"
else
  range="${to}"
  echo "Changes in this first release:"
fi
echo

log=$(git log --no-merges --format='%s' "$range" | grep -v '\[skip ci\]' || true)

section() {
  local title=$1 pattern=$2
  local lines
  lines=$(printf '%s\n' "$log" | grep -E "$pattern" || true)
  if [ -n "$lines" ]; then
    echo "### ${title}"
    echo
    printf '%s\n' "$lines" | sed -E 's/^[a-z]+(\([^)]*\))?!?: */- /'
    echo
  fi
}

section "Features" '^feat(\(|:|!)'
section "Fixes" '^fix(\(|:|!)'
section "Performance" '^perf(\(|:|!)'
section "Documentation" '^docs(\(|:|!)'
section "Build, CI and tests" '^(build|ci|test|chore|refactor|style)(\(|:|!)'
other=$(printf '%s\n' "$log" | grep -Ev '^(feat|fix|perf|docs|build|ci|test|chore|refactor|style)(\(|:|!)' | grep -v '^$' || true)
if [ -n "$other" ]; then
  echo "### Other"
  echo
  printf '%s\n' "$other" | sed 's/^/- /'
  echo
fi
