#!/usr/bin/env bash
set -euo pipefail

if [[ "${WIREVIEW_RELEASE_HIL:-}" != "1" ]]; then
  echo "Set WIREVIEW_RELEASE_HIL=1 to run attended release qualification." >&2
  exit 2
fi

for command in wireview jq sha256sum systemctl; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    echo "required command not found: ${command}" >&2
    exit 1
  fi
done

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
qualification_start="$(date --iso-8601=seconds)"
artifact_dir="${WIREVIEW_QUALIFICATION_DIR:-${project_dir}/target/release-qualification-${timestamp}}"
if [[ -z "${artifact_dir}" || "${artifact_dir}" == "/" ]]; then
  echo "unsafe qualification artifact directory" >&2
  exit 2
fi
install -d "${artifact_dir}"

configuration_started=0
restore_configuration() {
  if [[ "${configuration_started}" -ne 1 ]]; then
    return
  fi
  if wireview config show --json >"${artifact_dir}/configuration-current.json" 2>/dev/null; then
    jq -n \
      --slurpfile original "${artifact_dir}/configuration-original.json" \
      --slurpfile current "${artifact_dir}/configuration-current.json" \
      '{revision: $current[0].revision, settings: $original[0].settings}' \
      >"${artifact_dir}/configuration-restore.json"
    wireview config apply "${artifact_dir}/configuration-restore.json" \
      >"${artifact_dir}/configuration-restore.out" 2>&1 || true
  fi
}
trap restore_configuration EXIT

wireview version | tee "${artifact_dir}/wireview-version.txt"
wireview status | tee "${artifact_dir}/status-initial.txt"
wireview info --json | tee "${artifact_dir}/device-info-initial.json" >/dev/null
wireview telemetry --json | tee "${artifact_dir}/telemetry-initial.json" >/dev/null
wireview faults --json | tee "${artifact_dir}/faults-initial.json" >/dev/null
wireview config show --json | tee "${artifact_dir}/configuration-original.json" >/dev/null
jq -S .settings "${artifact_dir}/configuration-original.json" \
  >"${artifact_dir}/configuration-original-settings.json"
sha256sum "${artifact_dir}/configuration-original-settings.json" \
  >"${artifact_dir}/configuration-original-settings.sha256"

if [[ -n "${WIREVIEW_EXPECT_BUILD_ID:-}" ]] \
  && ! grep -Fq "build ${WIREVIEW_EXPECT_BUILD_ID})" \
    "${artifact_dir}/wireview-version.txt"; then
  echo "installed CLI does not match WIREVIEW_EXPECT_BUILD_ID=${WIREVIEW_EXPECT_BUILD_ID}" >&2
  exit 1
fi

if command -v pacman >/dev/null 2>&1; then
  pacman -Q wireviewd >"${artifact_dir}/package.txt" 2>/dev/null || true
elif command -v dpkg-query >/dev/null 2>&1; then
  dpkg-query -W wireviewd >"${artifact_dir}/package.txt" 2>/dev/null || true
elif command -v rpm >/dev/null 2>&1; then
  rpm -q wireviewd >"${artifact_dir}/package.txt" 2>/dev/null || true
fi
sha256sum "$(command -v wireview)" "$(command -v wireviewd)" \
  >"${artifact_dir}/installed-binaries.sha256"

history_output="${artifact_dir}/interrupted-history.raw"
wireview history --format raw --output "${history_output}" \
  >"${artifact_dir}/history-cancel.stdout" \
  2>"${artifact_dir}/history-cancel.stderr" &
history_pid=$!
history_active=0
for _ in {1..200}; do
  if wireview status >"${artifact_dir}/status-history-active.txt" 2>/dev/null \
    && grep -q 'display_paused=true' "${artifact_dir}/status-history-active.txt"; then
    history_active=1
    break
  fi
  if ! kill -0 "${history_pid}" 2>/dev/null; then
    break
  fi
  sleep 0.05
done
if [[ "${history_active}" -ne 1 ]]; then
  kill -INT "${history_pid}" 2>/dev/null || true
  wait "${history_pid}" 2>/dev/null || true
  echo "history dump did not become active" >&2
  exit 1
fi
kill -INT "${history_pid}"
set +e
wait "${history_pid}"
history_exit=$?
set -e
if [[ "${history_exit}" -ne 130 ]]; then
  echo "interrupted history exited with ${history_exit}, expected 130" >&2
  exit 1
fi
test ! -e "${history_output}"
wireview status | tee "${artifact_dir}/status-history-cleaned.txt"
grep -q 'state=ready' "${artifact_dir}/status-history-cleaned.txt"
grep -q 'display_paused=false' "${artifact_dir}/status-history-cleaned.txt"

if [[ "${WIREVIEW_RELEASE_CONFIG:-}" == "1" ]]; then
  configuration_started=1
  original_backlight="$(jq -r '.settings.backlight_percent' \
    "${artifact_dir}/configuration-original.json")"
  if [[ "${original_backlight}" -eq 100 ]]; then
    temporary_backlight=99
  else
    temporary_backlight=$((original_backlight + 1))
  fi
  wireview config set backlight_percent "${temporary_backlight}" \
    >"${artifact_dir}/configuration-temporary.out"
  test "$(wireview config get backlight_percent --json | jq -r .value)" \
    = "${temporary_backlight}"
  wireview config reload >"${artifact_dir}/configuration-reload.out"
  restore_configuration
  configuration_started=0
  wireview config show --json >"${artifact_dir}/configuration-after-test.json"
  diff -u \
    "${artifact_dir}/configuration-original-settings.json" \
    <(jq -S .settings "${artifact_dir}/configuration-after-test.json")
fi

if [[ "${WIREVIEW_RELEASE_SYSTEMD:-}" == "1" ]]; then
  sudo systemctl restart wireviewd.service
  wireview status >"${artifact_dir}/status-after-service-restart.txt"
  sudo systemctl stop wireviewd.service
  wireview status >"${artifact_dir}/status-after-socket-activation.txt"
  systemctl is-active --quiet wireviewd.socket
  systemctl is-active --quiet wireviewd.service
fi

if [[ "${WIREVIEW_RELEASE_DISCONNECT:-}" == "1" ]]; then
  if [[ ! -t 0 ]]; then
    echo "disconnect qualification requires an interactive terminal" >&2
    exit 2
  fi
  session_before="$(wireview status | sed -n 's/.*session=\\([0-9][0-9]*\\).*/\\1/p')"
  uid_before="$(jq -r .unique_id "${artifact_dir}/device-info-initial.json")"
  read -r -p \
    "Detach the WireView from this host (physical removal or attach it to the VM), then press Enter. "
  detached=0
  for _ in {1..200}; do
    wireview status >"${artifact_dir}/status-detached.txt" 2>/dev/null || true
    if grep -Eq 'state=(absent|recovering)' "${artifact_dir}/status-detached.txt"; then
      detached=1
      break
    fi
    sleep 0.1
  done
  if [[ "${detached}" -ne 1 ]]; then
    echo "daemon did not observe device detachment" >&2
    exit 1
  fi
  read -r -p \
    "Return the same WireView to this host, then press Enter. "
  reconnected=0
  for _ in {1..300}; do
    wireview status >"${artifact_dir}/status-reconnected.txt" 2>/dev/null || true
    if grep -q 'state=ready' "${artifact_dir}/status-reconnected.txt"; then
      reconnected=1
      break
    fi
    sleep 0.1
  done
  if [[ "${reconnected}" -ne 1 ]]; then
    echo "daemon did not reconnect to the device" >&2
    exit 1
  fi
  session_after="$(sed -n 's/.*session=\\([0-9][0-9]*\\).*/\\1/p' \
    "${artifact_dir}/status-reconnected.txt")"
  wireview info --json >"${artifact_dir}/device-info-reconnected.json"
  uid_after="$(jq -r .unique_id "${artifact_dir}/device-info-reconnected.json")"
  test "${session_after}" -gt "${session_before}"
  test "${uid_after}" = "${uid_before}"
fi

wireview telemetry --json >"${artifact_dir}/telemetry-final.json"
wireview config show --json >"${artifact_dir}/configuration-final.json"
diff -u \
  "${artifact_dir}/configuration-original-settings.json" \
  <(jq -S .settings "${artifact_dir}/configuration-final.json")
wireview status | tee "${artifact_dir}/status-final.txt"
systemctl --no-pager --full status wireviewd.socket wireviewd.service \
  >"${artifact_dir}/systemd-status.txt"
journalctl --no-pager -u wireviewd.service --since "${qualification_start}" \
  >"${artifact_dir}/wireviewd-journal.txt"
if journalctl --no-pager -q -u wireviewd.service \
  --since "${qualification_start}" -p err | grep -q .; then
  echo "wireviewd logged an error during qualification" >&2
  exit 1
fi

trap - EXIT
echo "Release qualification passed. Evidence: ${artifact_dir}"
