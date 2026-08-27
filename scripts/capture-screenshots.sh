#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output_dir="${1:-${project_dir}/docs/assets/screenshots}"
render_dir="${project_dir}/target/documentation-screenshots"

if [[ -z "${output_dir}" || "${output_dir}" == "/" || \
      "${output_dir}" == "${project_dir}" ]]; then
  echo "the screenshot output must be a dedicated directory" >&2
  exit 2
fi

for command in cargo magick; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    echo "required command not found: ${command}" >&2
    exit 1
  fi
done

install -d "${output_dir}" "${render_dir}"
cd "${project_dir}"

for page in overview graphs; do
  cargo run \
    --locked \
    --quiet \
    --package wireview-gui \
    --features screenshots \
    --bin wireview-screenshot \
    -- "${render_dir}/${page}.ppm" "${page}"
  magick "${render_dir}/${page}.ppm" \
    -strip \
    -define png:compression-level=9 \
    "${output_dir}/${page}.png"
done

echo "Screenshots written to ${output_dir}"
