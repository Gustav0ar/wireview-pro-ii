#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
stage_dir="${project_dir}/target/package-validation-root"
sbom_path="${project_dir}/target/package-validation.spdx.json"

cleanup() {
  rm -rf -- "${stage_dir}"
  rm -f -- "${sbom_path}"
}
trap cleanup EXIT

cd "${project_dir}"

bash -n \
  scripts/build-packages.sh \
  scripts/build-identity.sh \
  scripts/generate-sbom.sh \
  scripts/install-staged.sh \
  scripts/qualify-release.sh \
  scripts/smoke-varlink.sh \
  scripts/smoke-hardware.sh \
  scripts/soak-test.sh \
  scripts/test-build-identity.sh \
  packaging/arch/wireviewd.install
for script in packaging/debian/postinst packaging/debian/prerm packaging/debian/postrm; do
  sh -n "${script}"
done

bash scripts/test-build-identity.sh

build_identity="$(
  bash scripts/build-identity.sh 1785414000
)"
if [[ ! "${build_identity}" =~ ^(git|source)-[0-9a-f]{12}-20260730122000$ ]]; then
  echo "unexpected build identity: ${build_identity}" >&2
  exit 1
fi

udevadm verify --resolve-names=never \
  packaging/udev/70-wireview-pro-ii.rules
varlinkctl validate-idl \
  interfaces/io.github.Gustav0ar.WireView.varlink
cargo build --release --locked --bins
SOURCE_DATE_EPOCH=1785326400 \
  bash scripts/generate-sbom.sh "${sbom_path}"
python3 -m json.tool "${sbom_path}" >/dev/null
grep -Fq '"spdxVersion": "SPDX-2.3"' "${sbom_path}"
rm -rf -- "${stage_dir}"
"${project_dir}/scripts/install-staged.sh" "${stage_dir}"

for target in basic shutdown sockets sysinit; do
  {
    printf '%s\n' \
      '[Unit]' \
      "Description=Packaging validation ${target} target" \
      'DefaultDependencies=no'
  } >"${stage_dir}/usr/lib/systemd/system/${target}.target"
done

systemd-sysusers --dry-run --root="${stage_dir}" \
  "${stage_dir}/usr/lib/sysusers.d/wireview.conf" >/dev/null
systemd-analyze verify --root="${stage_dir}" \
  /usr/lib/systemd/system/wireviewd.service \
  /usr/lib/systemd/system/wireviewd.socket
grep -Fqx 'g wireview-client - -' \
  "${stage_dir}/usr/lib/sysusers.d/wireview.conf"
grep -Fqx 'SocketMode=0660' \
  "${stage_dir}/usr/lib/systemd/system/wireviewd.socket"
grep -Fqx 'SocketGroup=wireview-client' \
  "${stage_dir}/usr/lib/systemd/system/wireviewd.socket"
for limit in 'LimitNOFILE=128' 'MemoryHigh=96M' 'MemoryMax=128M' 'TasksMax=32'; do
  grep -Fqx "${limit}" \
    "${stage_dir}/usr/lib/systemd/system/wireviewd.service"
done

test "$(stat -c '%a' "${stage_dir}/usr/bin/wireviewd")" = "755"
test "$(stat -c '%a' "${stage_dir}/usr/bin/wireview")" = "755"
test "$(stat -c '%a' \
  "${stage_dir}/usr/lib/udev/rules.d/70-wireview-pro-ii.rules")" = "644"
test "$(stat -c '%a' \
  "${stage_dir}/usr/lib/systemd/system/wireviewd.socket")" = "644"
test "$(stat -c '%a' \
  "${stage_dir}/usr/share/varlink/interfaces/io.github.Gustav0ar.WireView.varlink")" = "644"
test "$(stat -c '%a' \
  "${stage_dir}/usr/share/doc/wireviewd/release-qualification.md")" = "644"
for document in usage operations development; do
  test "$(stat -c '%a' \
    "${stage_dir}/usr/share/doc/wireviewd/${document}.md")" = "644"
done
for asset in \
  usr/share/man/man1/wireview.1 \
  usr/share/bash-completion/completions/wireview \
  usr/share/zsh/site-functions/_wireview \
  usr/share/fish/vendor_completions.d/wireview.fish; do
  test "$(stat -c '%a' "${stage_dir}/${asset}")" = "644"
  grep -Fq "wireview" "${stage_dir}/${asset}"
  if grep -Fq "__generate-assets" "${stage_dir}/${asset}"; then
    echo "packaging-only command leaked into ${asset}" >&2
    exit 1
  fi
done
grep -Fq \
  '%doc /usr/share/doc/wireviewd/release-qualification.md' \
  packaging/rpm/wireviewd.spec.in
grep -Fq '%doc /usr/share/doc/wireviewd/usage.md' \
  packaging/rpm/wireviewd.spec.in
grep -Fq '%{_mandir}/man1/wireview.1*' packaging/rpm/wireviewd.spec.in
test "$(find "${stage_dir}" -type f -perm /0002 -print -quit)" = ""

echo "Packaging validation passed"
