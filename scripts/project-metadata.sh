#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
field="${1:-}"

case "${field}" in
  version|repository) ;;
  *)
    echo "usage: $0 version|repository" >&2
    exit 2
    ;;
esac

command -v cargo >/dev/null 2>&1 || {
  echo "cargo is required to read project metadata" >&2
  exit 1
}
command -v python3 >/dev/null 2>&1 || {
  echo "python3 is required to read project metadata" >&2
  exit 1
}

cd "${project_dir}"
cargo metadata --locked --no-deps --format-version 1 |
  python3 -c '
import json
import pathlib
import sys

field = sys.argv[1]
project_dir = pathlib.Path(sys.argv[2]).resolve()
metadata = json.load(sys.stdin)
root_manifest = project_dir / "Cargo.toml"
package = next(
    candidate
    for candidate in metadata["packages"]
    if pathlib.Path(candidate["manifest_path"]).resolve() == root_manifest
)
value = package[field]
if not isinstance(value, str) or not value:
    raise SystemExit(f"root package has no {field}")
print(value)
' "${field}" "${project_dir}"
