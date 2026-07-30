#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output="${1:-${project_dir}/dist/wireviewd.spdx.json}"
source_epoch="${SOURCE_DATE_EPOCH:-$(date +%s)}"

command -v cargo >/dev/null 2>&1 || {
  echo "cargo is required to generate the SBOM" >&2
  exit 1
}
command -v python3 >/dev/null 2>&1 || {
  echo "python3 is required to generate the SBOM" >&2
  exit 1
}

install -d "$(dirname "${output}")"
cd "${project_dir}"
cargo metadata --locked --format-version 1 |
  SOURCE_DATE_EPOCH="${source_epoch}" python3 -c '
import datetime
import json
import os
import re
import sys
import uuid

metadata = json.load(sys.stdin)
packages = metadata["packages"]
root_id = metadata["resolve"]["root"]
root = next(package for package in packages if package["id"] == root_id)
root_name = root["name"]
root_version = root["version"]

def spdx_id(package_id):
    value = re.sub(r"[^A-Za-z0-9.-]", "-", package_id)
    return "SPDXRef-" + value

created = datetime.datetime.fromtimestamp(
    int(os.environ["SOURCE_DATE_EPOCH"]), datetime.timezone.utc
).strftime("%Y-%m-%dT%H:%M:%SZ")
namespace_seed = f"{root_name}:{root_version}:{created}"
document = {
    "spdxVersion": "SPDX-2.3",
    "dataLicense": "CC0-1.0",
    "SPDXID": "SPDXRef-DOCUMENT",
    "name": f"{root_name}-{root_version}",
    "documentNamespace": (
        "https://github.com/Gustav0ar/wireview-pro-ii/spdx/"
        + str(uuid.uuid5(uuid.NAMESPACE_URL, namespace_seed))
    ),
    "creationInfo": {
        "created": created,
        "creators": ["Tool: wireviewd/scripts/generate-sbom.sh"],
    },
    "packages": [],
    "relationships": [
        {
            "spdxElementId": "SPDXRef-DOCUMENT",
            "relationshipType": "DESCRIBES",
            "relatedSpdxElement": spdx_id(root_id),
        }
    ],
}

for package in packages:
    license_value = package.get("license") or "NOASSERTION"
    source = package.get("source")
    download = source[0] if source else "NOASSERTION"
    document["packages"].append(
        {
            "name": package["name"],
            "SPDXID": spdx_id(package["id"]),
            "versionInfo": package["version"],
            "downloadLocation": download,
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": license_value,
            "copyrightText": "NOASSERTION",
        }
    )

nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
for package_id, node in nodes.items():
    for dependency in node["deps"]:
        document["relationships"].append(
            {
                "spdxElementId": spdx_id(package_id),
                "relationshipType": "DEPENDS_ON",
                "relatedSpdxElement": spdx_id(dependency["pkg"]),
            }
        )

json.dump(document, sys.stdout, indent=2, sort_keys=True)
sys.stdout.write("\n")
' >"${output}"

echo "SPDX SBOM written to ${output}"
