#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 || ! "$1" =~ ^[0-9]+$ ]]; then
  echo "usage: build-identity.sh SOURCE_DATE_EPOCH" >&2
  exit 2
fi

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
build_date="$(date --utc --date="@$1" '+%Y%m%d%H%M%S')"

cd "${project_dir}"
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
    if [[ "${WIREVIEW_ALLOW_DIRTY:-}" != "1" ]]; then
      echo "refusing to identify a dirty Git checkout as a clean commit" >&2
      echo "commit/stash changes, set WIREVIEW_BUILD_ID, or explicitly set WIREVIEW_ALLOW_DIRTY=1" >&2
      exit 1
    fi
    source_kind="source"
    source_sha="$(
      find . -type f \
        ! -path './.git/*' \
        ! -path './target/*' \
        ! -path './dist/*' \
        ! -path './fuzz/target/*' \
        ! -path './fuzz/artifacts/*' \
        ! -path './fuzz/corpus/*' \
        -print0 \
        | LC_ALL=C sort -z \
        | xargs -0 sha256sum \
        | sha256sum \
        | cut -c1-12
    )"
  else
    source_kind="git"
    source_sha="$(git rev-parse --short=12 HEAD)"
  fi
else
  source_kind="source"
  source_sha="$(
    find . -type f \
      ! -path './target/*' \
      ! -path './dist/*' \
      ! -path './fuzz/target/*' \
      ! -path './fuzz/artifacts/*' \
      ! -path './fuzz/corpus/*' \
      -print0 \
      | LC_ALL=C sort -z \
      | xargs -0 sha256sum \
      | sha256sum \
      | cut -c1-12
  )"
fi

printf '%s-%s-%s\n' "${source_kind}" "${source_sha}" "${build_date}"
