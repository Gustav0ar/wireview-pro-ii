#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_dir="$(mktemp -d)"

cleanup() {
  rm -rf -- "${test_dir}"
}
trap cleanup EXIT

install -d "${test_dir}/scripts"
install -m 0755 "${project_dir}/scripts/build-identity.sh" \
  "${test_dir}/scripts/build-identity.sh"
printf '%s\n' 'identity fixture' >"${test_dir}/source.txt"

git -C "${test_dir}" init --quiet
git -C "${test_dir}" config user.name "wireviewd tests"
git -C "${test_dir}" config user.email "tests@example.invalid"
git -C "${test_dir}" add .
GIT_AUTHOR_DATE=1785414000 GIT_COMMITTER_DATE=1785414000 \
  git -C "${test_dir}" commit --quiet -m "fixture"

commit_sha="$(git -C "${test_dir}" rev-parse --short=12 HEAD)"
clean_identity="$(
  bash "${test_dir}/scripts/build-identity.sh" 1785414000
)"
test "${clean_identity}" = "git-${commit_sha}-20260730122000"

printf '%s\n' 'dirty change' >>"${test_dir}/source.txt"
if bash "${test_dir}/scripts/build-identity.sh" 1785414000 \
  >"${test_dir}/dirty.stdout" 2>"${test_dir}/dirty.stderr"; then
  echo "dirty Git checkout unexpectedly received a clean build identity" >&2
  exit 1
fi
grep -Fq "refusing to identify a dirty Git checkout" \
  "${test_dir}/dirty.stderr"

dirty_identity="$(
  WIREVIEW_ALLOW_DIRTY=1 \
    bash "${test_dir}/scripts/build-identity.sh" 1785414000
)"
if [[ ! "${dirty_identity}" =~ ^source-[0-9a-f]{12}-20260730122000$ ]]; then
  echo "unexpected dirty-source identity: ${dirty_identity}" >&2
  exit 1
fi

echo "Build identity validation passed"
