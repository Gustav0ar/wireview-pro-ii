#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist_dir="${WIREVIEW_DIST_DIR:-${project_dir}/dist}"
work_dir="${project_dir}/target/packaging"

if [[ -z "${dist_dir}" || "${dist_dir}" == "/" || "${dist_dir}" == "${project_dir}" ]]; then
  echo "WIREVIEW_DIST_DIR must name a dedicated output directory" >&2
  exit 2
fi

if [[ "$#" -eq 0 ]]; then
  formats=(deb rpm arch)
else
  formats=("$@")
fi

for format in "${formats[@]}"; do
  case "${format}" in
    deb|rpm|arch) ;;
    *)
      echo "unknown package format: ${format} (expected deb, rpm, or arch)" >&2
      exit 2
      ;;
  esac
done

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "required command not found: $1" >&2
    exit 1
  fi
}

require_command cargo
require_command python3
require_command sha256sum
require_command tar
for format in "${formats[@]}"; do
  case "${format}" in
    deb) require_command dpkg-deb ;;
    rpm)
      require_command rpmbuild
      require_command rpm
      ;;
    arch)
      require_command makepkg
      require_command pacman
      ;;
  esac
done

cd "${project_dir}"
version="$(bash scripts/project-metadata.sh version)"
package_version="${WIREVIEW_PACKAGE_VERSION:-${version}}"
project_url="$(bash scripts/project-metadata.sh repository)"
if [[ -z "${version}" || -z "${project_url}" ]]; then
  echo "failed to read package version or repository from Cargo.toml" >&2
  exit 1
fi
if [[ ! "${package_version}" =~ ^[0-9]+([.][0-9]+)*([+._~-][A-Za-z0-9]+)*$ ]]; then
  echo "WIREVIEW_PACKAGE_VERSION is not a portable package version: ${package_version}" >&2
  exit 2
fi
machine="$(uname -m)"
case "${machine}" in
  x86_64)
    deb_arch="amd64"
    rpm_arch="x86_64"
    ;;
  aarch64)
    deb_arch="arm64"
    rpm_arch="aarch64"
    ;;
  *)
    echo "unsupported package architecture: ${machine}" >&2
    exit 1
    ;;
esac

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)"
  else
    SOURCE_DATE_EPOCH="$(date +%s)"
  fi
fi
export SOURCE_DATE_EPOCH
if [[ -z "${WIREVIEW_BUILD_ID:-}" ]]; then
  if [[ "${WIREVIEW_SKIP_BUILD:-}" == "1" && -x target/release/wireview ]]; then
    WIREVIEW_BUILD_ID="$(
      target/release/wireview version \
        | sed -n 's/.*(build \(.*\))$/\1/p'
    )"
  else
    WIREVIEW_BUILD_ID="$(
      bash "${project_dir}/scripts/build-identity.sh" "${SOURCE_DATE_EPOCH}"
    )"
  fi
fi
if [[ ! "${WIREVIEW_BUILD_ID}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]]; then
  echo "WIREVIEW_BUILD_ID must be 1-64 portable identifier characters" >&2
  exit 2
fi
export WIREVIEW_BUILD_ID
rm -rf -- "${work_dir}"
if [[ "${WIREVIEW_KEEP_DIST:-}" != "1" ]]; then
  rm -rf -- "${dist_dir}"
fi
install -d "${work_dir}/root" "${dist_dir}"

if [[ "${WIREVIEW_SKIP_BUILD:-}" != "1" ]]; then
  cargo build --release --locked --workspace --bins
elif [[ ! -x target/release/wireviewd || ! -x target/release/wireview ||
        ! -x target/release/wireview-gui ]]; then
  echo "WIREVIEW_SKIP_BUILD=1 requires target/release/wireviewd, wireview, and wireview-gui" >&2
  exit 1
fi
if ! target/release/wireview version \
  | grep -Fqx "wireview ${version} (build ${WIREVIEW_BUILD_ID})"; then
  echo "release binary does not contain WIREVIEW_BUILD_ID=${WIREVIEW_BUILD_ID}" >&2
  exit 1
fi
if ! target/release/wireview-gui --version \
  | grep -Fqx "wireview-gui ${version} (build ${WIREVIEW_BUILD_ID})"; then
  echo "desktop binary does not contain WIREVIEW_BUILD_ID=${WIREVIEW_BUILD_ID}" >&2
  exit 1
fi
"${project_dir}/scripts/install-staged.sh" "${work_dir}/root"
find "${work_dir}/root" -print0 \
  | xargs -0 touch --date="@${SOURCE_DATE_EPOCH}"

build_deb() {
  local root="${work_dir}/deb/root"
  local control_dir="${root}/DEBIAN"
  local package="${dist_dir}/wireviewd_${package_version}-1_${deb_arch}.deb"
  local installed_size

  install -d "${root}"
  cp -a "${work_dir}/root/." "${root}/"
  install -d "${control_dir}"
  installed_size="$(du -sk "${root}/usr" | awk '{print $1}')"
  {
    printf '%s\n' \
      'Package: wireviewd' \
      "Version: ${package_version}-1" \
      'Section: utils' \
      'Priority: optional' \
      "Architecture: ${deb_arch}" \
      'Maintainer: wireviewd contributors <wireviewd-maintainers@users.noreply.github.com>' \
      "Homepage: ${project_url}" \
      "Installed-Size: ${installed_size}" \
      'Depends: libc6 (>= 2.34), libfontconfig1, libudev1, libwayland-client0, libx11-6, libx11-xcb1, libxcursor1, libxi6, libxkbcommon0, libxkbcommon-x11-0, systemd, udev' \
      'Description: Linux tools for the Thermal Grizzly WireView Pro II' \
      ' Provides a native desktop app, a command-line client, and a daemon for' \
      ' telemetry, validated configuration, and verified device control.'
  } >"${control_dir}/control"
  install -m 0755 packaging/debian/postinst "${control_dir}/postinst"
  install -m 0755 packaging/debian/prerm "${control_dir}/prerm"
  install -m 0755 packaging/debian/postrm "${control_dir}/postrm"
  install -m 0644 packaging/debian/copyright \
    "${root}/usr/share/doc/wireviewd/copyright"

  find "${root}" -print0 | xargs -0 touch --date="@${SOURCE_DATE_EPOCH}"
  dpkg-deb --root-owner-group --build -Zxz -z9 "${root}" "${package}"
  dpkg-deb --info "${package}" >/dev/null
  dpkg-deb --contents "${package}" >/dev/null
}

build_rpm() {
  local top="${work_dir}/rpm"
  local rpm_db="${work_dir}/rpmdb"
  local spec="${top}/SPECS/wireviewd.spec"
  local rpm_date

  install -d \
    "${top}/BUILD" \
    "${top}/BUILDROOT" \
    "${top}/RPMS" \
    "${top}/SOURCES" \
    "${top}/SPECS" \
    "${top}/SRPMS" \
    "${rpm_db}"
  rpm --dbpath "${rpm_db}" --initdb
  rpm_date="$(LC_ALL=C date --date="@${SOURCE_DATE_EPOCH}" '+%a %b %d %Y')"
  sed \
    -e "s|@VERSION@|${package_version}|g" \
    -e "s|@RPM_DATE@|${rpm_date}|g" \
    -e "s|@PROJECT_URL@|${project_url}|g" \
    -e "s|@STAGE_ROOT@|${work_dir}/root|g" \
    packaging/rpm/wireviewd.spec.in >"${spec}"
  rpmbuild -bb \
    --define "__brp_strip %{nil}" \
    --define "__brp_strip_comment_note %{nil}" \
    --define "__brp_strip_static_archive %{nil}" \
    --define "_dbpath ${rpm_db}" \
    --define "_buildhost reproducible" \
    --define "_topdir ${top}" \
    --define "_target_cpu ${rpm_arch}" \
    --define "use_source_date_epoch_as_buildtime 1" \
    "${spec}"
  find "${top}/RPMS" -type f -name '*.rpm' -exec cp -a {} "${dist_dir}/" \;
  while IFS= read -r package; do
    rpm --dbpath "${rpm_db}" --query --package --info "${package}" >/dev/null
    rpm --dbpath "${rpm_db}" --query --package --list "${package}" >/dev/null
  done < <(find "${dist_dir}" -maxdepth 1 -type f -name '*.rpm' -print)
}

build_arch() {
  local root="${work_dir}/arch"
  local cargo_bin_dir
  local package_path
  local root_sha256

  install -d "${root}"
  tar \
    --sort=name \
    --mtime="@${SOURCE_DATE_EPOCH}" \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -C "${work_dir}" \
    -cf "${root}/wireviewd-root.tar" \
    root
  root_sha256="$(sha256sum "${root}/wireviewd-root.tar" | awk '{print $1}')"
  sed \
    -e "s|@VERSION@|${package_version}|g" \
    -e "s|@PROJECT_URL@|${project_url}|g" \
    -e "s|@ROOT_SHA256@|${root_sha256}|g" \
    packaging/arch/PKGBUILD.in >"${root}/PKGBUILD"
  install -m 0644 packaging/arch/wireviewd.install \
    "${root}/wireviewd.install"
  cp /etc/makepkg.conf "${root}/makepkg.conf"
  printf '%s\n' \
    "PACKAGER='wireviewd contributors <wireviewd-maintainers@users.noreply.github.com>'" \
    >>"${root}/makepkg.conf"
  cargo_bin_dir="$(dirname "$(command -v cargo)")"
  package_path="${cargo_bin_dir}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
  (
    cd "${root}"
    PATH="${package_path}" PKGDEST="${dist_dir}" makepkg \
      --clean \
      --config "${root}/makepkg.conf" \
      --force \
      --noconfirm \
      --nodeps
  )
  while IFS= read -r package; do
    pacman --query --info --file "${package}" >/dev/null
    pacman --query --list --file "${package}" >/dev/null
  done < <(find "${dist_dir}" -maxdepth 1 -type f -name '*.pkg.tar.*' -print)
}

for format in "${formats[@]}"; do
  "build_${format}"
done

bash "${project_dir}/scripts/generate-sbom.sh" \
  "${dist_dir}/wireviewd.spdx.json"

(
  cd "${dist_dir}"
  find . -maxdepth 1 -type f \
    \( -name '*.deb' -o -name '*.rpm' -o -name '*.pkg.tar.*' \
       -o -name '*.spdx.json' \) \
    -printf '%P\n' \
    | sort \
    | xargs -r sha256sum >SHA256SUMS
)

echo "Packages written to ${dist_dir}:"
find "${dist_dir}" -maxdepth 1 -type f -printf '  %f\n' | sort
