#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 || -z "$1" || "$1" == "/" ]]; then
  echo "usage: $0 DESTDIR" >&2
  exit 2
fi

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
destdir="$1"

for binary in wireviewd wireview wireview-gui; do
  if [[ ! -x "${project_dir}/target/release/${binary}" ]]; then
    echo "missing release binary: target/release/${binary}" >&2
    exit 1
  fi
done

install -d \
  "${destdir}/usr/bin" \
  "${destdir}/usr/lib/systemd/system" \
  "${destdir}/usr/lib/sysusers.d" \
  "${destdir}/usr/lib/udev/rules.d" \
  "${destdir}/usr/share/applications" \
  "${destdir}/usr/share/icons/hicolor/scalable/apps" \
  "${destdir}/usr/share/varlink/interfaces" \
  "${destdir}/usr/share/man/man1" \
  "${destdir}/usr/share/bash-completion/completions" \
  "${destdir}/usr/share/zsh/site-functions" \
  "${destdir}/usr/share/fish/vendor_completions.d" \
  "${destdir}/usr/share/doc/wireviewd" \
  "${destdir}/usr/share/licenses/wireviewd"

install -m 0755 "${project_dir}/target/release/wireviewd" \
  "${destdir}/usr/bin/wireviewd"
install -m 0755 "${project_dir}/target/release/wireview" \
  "${destdir}/usr/bin/wireview"
install -m 0755 "${project_dir}/target/release/wireview-gui" \
  "${destdir}/usr/bin/wireview-gui"
"${destdir}/usr/bin/wireview" __generate-assets "${destdir}"
chmod 0644 \
  "${destdir}/usr/share/man/man1/wireview.1" \
  "${destdir}/usr/share/bash-completion/completions/wireview" \
  "${destdir}/usr/share/zsh/site-functions/_wireview" \
  "${destdir}/usr/share/fish/vendor_completions.d/wireview.fish"
install -m 0644 "${project_dir}/packaging/systemd/wireviewd.service" \
  "${destdir}/usr/lib/systemd/system/wireviewd.service"
install -m 0644 "${project_dir}/packaging/systemd/wireviewd.socket" \
  "${destdir}/usr/lib/systemd/system/wireviewd.socket"
install -m 0644 "${project_dir}/packaging/sysusers.d/wireview.conf" \
  "${destdir}/usr/lib/sysusers.d/wireview.conf"
install -m 0644 "${project_dir}/packaging/udev/70-wireview-pro-ii.rules" \
  "${destdir}/usr/lib/udev/rules.d/70-wireview-pro-ii.rules"
install -m 0644 \
  "${project_dir}/packaging/applications/io.github.Gustav0ar.WireView.desktop" \
  "${destdir}/usr/share/applications/io.github.Gustav0ar.WireView.desktop"
install -m 0644 \
  "${project_dir}/packaging/icons/hicolor/scalable/apps/io.github.Gustav0ar.WireView.svg" \
  "${destdir}/usr/share/icons/hicolor/scalable/apps/io.github.Gustav0ar.WireView.svg"
install -m 0644 \
  "${project_dir}/interfaces/io.github.Gustav0ar.WireView.varlink" \
  "${destdir}/usr/share/varlink/interfaces/io.github.Gustav0ar.WireView.varlink"
install -m 0644 "${project_dir}/README.md" \
  "${destdir}/usr/share/doc/wireviewd/README.md"
install -m 0644 "${project_dir}/docs/protocol.md" \
  "${destdir}/usr/share/doc/wireviewd/protocol.md"
install -m 0644 "${project_dir}/docs/compatibility.md" \
  "${destdir}/usr/share/doc/wireviewd/compatibility.md"
install -m 0644 "${project_dir}/docs/usage.md" \
  "${destdir}/usr/share/doc/wireviewd/usage.md"
install -m 0644 "${project_dir}/docs/desktop.md" \
  "${destdir}/usr/share/doc/wireviewd/desktop.md"
install -m 0644 "${project_dir}/docs/operations.md" \
  "${destdir}/usr/share/doc/wireviewd/operations.md"
install -m 0644 "${project_dir}/docs/development.md" \
  "${destdir}/usr/share/doc/wireviewd/development.md"
install -m 0644 "${project_dir}/docs/varlink.md" \
  "${destdir}/usr/share/doc/wireviewd/varlink.md"
install -m 0644 "${project_dir}/docs/release-qualification.md" \
  "${destdir}/usr/share/doc/wireviewd/release-qualification.md"
install -m 0644 "${project_dir}/LICENSE" \
  "${destdir}/usr/share/licenses/wireviewd/LICENSE"
