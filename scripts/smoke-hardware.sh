#!/usr/bin/env bash
set -euo pipefail

if [[ "${WIREVIEW_HIL:-}" != "1" ]]; then
  echo "Set WIREVIEW_HIL=1 to run the opt-in hardware test." >&2
  exit 2
fi

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
artifact_dir="$(mktemp -d)"
socket_path="${artifact_dir}/wireview.varlink"
daemon_pid=""
monitor_pid=""
configuration_changed=0

cleanup() {
  if [[ -n "${daemon_pid}" && "${configuration_changed}" -eq 1 ]]; then
    ./target/debug/wireview --socket "${socket_path}" config reload \
      >/dev/null 2>&1 || true
    ./target/debug/wireview --socket "${socket_path}" screen main \
      >/dev/null 2>&1 || true
  fi
  if [[ -n "${monitor_pid}" ]]; then
    kill "${monitor_pid}" 2>/dev/null || true
    wait "${monitor_pid}" 2>/dev/null || true
  fi
  if [[ -n "${daemon_pid}" ]]; then
    kill -INT "${daemon_pid}" 2>/dev/null || true
    wait "${daemon_pid}" 2>/dev/null || true
  fi
  rm -rf -- "${artifact_dir}"
}
trap cleanup EXIT

cd "${project_dir}"
if [[ "${WIREVIEW_SKIP_BUILD:-}" != "1" ]]; then
  cargo build --bins
fi

./target/debug/wireviewd \
  --socket "${socket_path}" \
  --poll-ms 250 \
  --discovery-ms 250 \
  >"${artifact_dir}/daemon.log" 2>&1 &
daemon_pid=$!

ready=0
for _ in {1..40}; do
  if ./target/debug/wireview --socket "${socket_path}" status \
    >"${artifact_dir}/status.out" 2>/dev/null \
    && grep -q '^state=ready ' "${artifact_dir}/status.out"; then
    ready=1
    break
  fi
  sleep 0.125
done

if [[ "${ready}" -ne 1 ]]; then
  cat "${artifact_dir}/status.out" >&2 2>/dev/null || true
  cat "${artifact_dir}/daemon.log" >&2
  exit 1
fi

./target/debug/wireview --socket "${socket_path}" telemetry --json \
  >"${artifact_dir}/telemetry.json"
./target/debug/wireview --socket "${socket_path}" telemetry \
  >"${artifact_dir}/telemetry.out"
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-original.json"
if [[ "${WIREVIEW_HIL_CONFIG_MUTATION:-}" == "1" ]]; then
  jq '
    .settings.backlight_percent = (
      if .settings.backlight_percent == 100
      then 99
      else .settings.backlight_percent + 1
      end
    )
  ' "${artifact_dir}/configuration-original.json" \
    >"${artifact_dir}/configuration-temporary.json"
  ./target/debug/wireview --socket "${socket_path}" config apply \
    "${artifact_dir}/configuration-temporary.json" --json \
    >"${artifact_dir}/configuration-apply-result.json"
  configuration_changed=1
  jq -e '.success == true and (.message | length > 0)' \
    "${artifact_dir}/configuration-apply-result.json" >/dev/null
  ./target/debug/wireview --socket "${socket_path}" config show --json \
    >"${artifact_dir}/configuration-applied.json"
  diff -u <(jq -S .settings "${artifact_dir}/configuration-temporary.json") \
    <(jq -S .settings "${artifact_dir}/configuration-applied.json")
  ./target/debug/wireview --socket "${socket_path}" config reload --json \
    >"${artifact_dir}/configuration-reload-result.json"
  jq -e '.success == true and (.message | length > 0)' \
    "${artifact_dir}/configuration-reload-result.json" >/dev/null
  ./target/debug/wireview --socket "${socket_path}" config show --json \
    >"${artifact_dir}/configuration-restored.json"
  configuration_changed=0
  diff -u "${artifact_dir}/configuration-original.json" \
    "${artifact_dir}/configuration-restored.json"
fi
./target/debug/wireview --socket "${socket_path}" debug monitor --count 4 \
  >"${artifact_dir}/events.jsonl" &
monitor_pid=$!

for screen in main current temp status simple same temperature main; do
  ./target/debug/wireview --socket "${socket_path}" screen "${screen}" \
    >>"${artifact_dir}/screens.out"
done
wait "${monitor_pid}"
monitor_pid=""

if ./target/debug/wireview --socket "${socket_path}" screen invalid \
  >"${artifact_dir}/invalid-screen.out" 2>&1; then
  echo "invalid screen command unexpectedly succeeded" >&2
  exit 1
fi

jq -e '
  . as $telemetry
  | .session_id == 1
  and (.sequence > 0)
  and (.observed_at_ms > 0)
  and (.pin_currents_a | length) == 6
  and (.pin_voltages_v | length) == 6
  and (.pin_power_w | length) == 6
  and (.vdd_v > 0)
  and (.avg_voltage_v > 0)
  and (.total_current_a >= 0)
  and (.total_power_w >= 0)
  and (.fan_duty_percent >= 0 and .fan_duty_percent <= 100)
  and ([150, 300, 450, 600] | index($telemetry.cable_capability_w) != null)
  and (.stale == false)
' "${artifact_dir}/telemetry.json" >/dev/null
for label in 'Connection: Connected' 'Last updated:' 'Average voltage:' 'Total current:' \
  'Total power:' 'Internal supply (VDD):' 'Cable power rating:' 'Fan duty:' \
  'Connector pins' 'Pin 1:' 'Pin 6:' 'Temperatures' 'Onboard input:' \
  'Onboard output:' 'External sensor 1:' 'External sensor 2:' 'Faults' \
  'Active:' 'Logged:'; do
  grep -Fq "${label}" "${artifact_dir}/telemetry.out"
done
diff -u <(printf '%s\n' Main Current Temp Status Simple Same Temp Main) \
  "${artifact_dir}/screens.out"
grep -Fxq 'invalid argument: unknown screen "invalid"' \
  "${artifact_dir}/invalid-screen.out"
jq -s -e '
  length == 4
  and all(.[]; .session_id == 1 and .sequence > 0)
' "${artifact_dir}/events.jsonl" >/dev/null

echo "status:"
cat "${artifact_dir}/status.out"
echo "telemetry:"
cat "${artifact_dir}/telemetry.out"
echo "configuration:"
./target/debug/wireview --socket "${socket_path}" config show
jq '{
  session_id,
  sequence,
  stale,
  avg_voltage_v,
  total_current_a,
  total_power_w,
  fan_duty_percent,
  cable_capability_w,
  input_temp_c,
  output_temp_c,
  active_faults,
  logged_faults
}' "${artifact_dir}/telemetry.json"

./target/debug/wireview --socket "${socket_path}" screen main >/dev/null
if [[ "${configuration_changed}" -eq 1 ]]; then
  ./target/debug/wireview --socket "${socket_path}" config reload >/dev/null
  configuration_changed=0
fi
kill -INT "${daemon_pid}"
wait "${daemon_pid}"
daemon_pid=""
trap - EXIT
rm -rf -- "${artifact_dir}"

echo "Hardware smoke test passed"
