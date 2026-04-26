#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

version="$(cargo metadata --format-version=1 --no-deps \
  | jq -r '.packages[] | select(.name == "cargo-context-core") | .version')"

if [[ -z "$version" || "$version" == "null" ]]; then
  echo "failed to determine workspace crate version" >&2
  exit 1
fi

for crate in cargo-context-core cargo-context-scrub cargo-context-cli cargo-context-mcp; do
  crate_version="$(cargo metadata --format-version=1 --no-deps \
    | jq -r --arg crate "$crate" '.packages[] | select(.name == $crate) | .version')"
  if [[ "$crate_version" != "$version" ]]; then
    echo "$crate version $crate_version does not match workspace version $version" >&2
    exit 1
  fi
done

if ! rg -q "^## \\[$version\\]" CHANGELOG.md; then
  echo "CHANGELOG.md is missing an entry for $version" >&2
  exit 1
fi

if [[ -n "${GITHUB_REF_NAME:-}" && "${GITHUB_REF_TYPE:-}" == "tag" ]]; then
  expected="v$version"
  if [[ "$GITHUB_REF_NAME" != "$expected" ]]; then
    echo "tag $GITHUB_REF_NAME does not match workspace version $version (expected $expected)" >&2
    exit 1
  fi
fi

echo "release version guard ok: $version"
