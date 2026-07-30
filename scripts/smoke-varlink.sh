#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
artifact_dir="$(mktemp -d)"
socket_path="${artifact_dir}/wireview.varlink"
daemon_pid=""
monitor_pid=""

cleanup() {
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
cargo build --bins

start_daemon() {
  local mode="${1:-direct}"
  if [[ "${mode}" == "activation" ]]; then
    systemd-socket-activate \
      --now \
      --listen="${socket_path}" \
      --fdname=wireviewd \
      ./target/debug/wireviewd \
      --mock \
      --poll-ms 100 \
      >"${artifact_dir}/daemon.log" 2>&1 &
  else
    ./target/debug/wireviewd \
      --mock \
      --socket "${socket_path}" \
      --poll-ms 100 \
      >"${artifact_dir}/daemon.log" 2>&1 &
  fi
  daemon_pid=$!

  local ready=0
  for _ in {1..30}; do
    if ./target/debug/wireview --socket "${socket_path}" status \
      >"${artifact_dir}/status.out" 2>/dev/null; then
      ready=1
      break
    fi
    sleep 0.1
  done
  [[ "${ready}" -eq 1 ]]
}

stop_daemon() {
  kill -INT "${daemon_pid}"
  wait "${daemon_pid}"
  daemon_pid=""
}

start_daemon
./target/debug/wireview --socket "${socket_path}" telemetry --json \
  >"${artifact_dir}/direct-telemetry.json"
./target/debug/wireview --socket "${socket_path}" screen main \
  >"${artifact_dir}/direct-screen.out"
grep -q '^Main$' "${artifact_dir}/direct-screen.out"
./target/debug/wireview --socket "${socket_path}" debug reboot-device --yes \
  >"${artifact_dir}/direct-reboot.out"
grep -q '^Device reboot command sent' "${artifact_dir}/direct-reboot.out"
./target/debug/wireview --socket "${socket_path}" status \
  >"${artifact_dir}/direct-reboot-status.out"
grep -q 'state=absent' "${artifact_dir}/direct-reboot-status.out"
stop_daemon

start_daemon activation
varlinkctl --no-ask-password info "unix:${socket_path}" \
  >"${artifact_dir}/varlink-info.out"
varlinkctl --no-ask-password list-methods \
  "unix:${socket_path}" io.github.Gustav0ar.WireView \
  >"${artifact_dir}/varlink-methods.out"
grep -q 'RebootDevice' "${artifact_dir}/varlink-methods.out"
for method in GetDeviceInfo ClearFaults BeginHistoryDump ReadHistoryDumpChunk \
  EndHistoryDump GetPollInterval SetPollInterval PauseDisplay ResumeDisplay; do
  grep -q "${method}" "${artifact_dir}/varlink-methods.out"
done
varlinkctl --no-ask-password call \
  "unix:${socket_path}" \
  io.github.Gustav0ar.WireView.GetStatus \
  '{}' \
  >"${artifact_dir}/varlink-status.json"
./target/debug/wireview --socket "${socket_path}" telemetry --json \
  >"${artifact_dir}/telemetry.json"
./target/debug/wireview --socket "${socket_path}" telemetry \
  >"${artifact_dir}/telemetry.out"
./target/debug/wireview --socket "${socket_path}" info --json \
  >"${artifact_dir}/device-info.json"
./target/debug/wireview --socket "${socket_path}" faults --json \
  >"${artifact_dir}/faults.json"
./target/debug/wireview --socket "${socket_path}" debug poll-interval \
  >"${artifact_dir}/poll-original.out"
./target/debug/wireview --socket "${socket_path}" debug poll-interval 250 \
  >"${artifact_dir}/poll-updated.out"
./target/debug/wireview --socket "${socket_path}" debug poll-interval 100 \
  >"${artifact_dir}/poll-restored.out"
./target/debug/wireview --socket "${socket_path}" debug pause-display 1 \
  >"${artifact_dir}/display-pause.out"
grep -q '^Display updates paused for up to ' "${artifact_dir}/display-pause.out"
./target/debug/wireview --socket "${socket_path}" status \
  >"${artifact_dir}/display-pause-status.out"
grep -q 'display_paused=true' "${artifact_dir}/display-pause-status.out"
./target/debug/wireview --socket "${socket_path}" debug resume-display \
  >"${artifact_dir}/display-resume.out"
grep -q '^Display updates resumed.$' "${artifact_dir}/display-resume.out"
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-original.json"
./target/debug/wireview --socket "${socket_path}" config set \
  friendly_name "revision race" \
  >"${artifact_dir}/configuration-revision-race.out"
original_configuration="$(jq -c .settings "${artifact_dir}/configuration-original.json")"
original_revision="$(jq -r .revision "${artifact_dir}/configuration-original.json")"
jq -cn --arg configuration_json "${original_configuration}" \
  --arg revision "${original_revision}" \
  '{configuration: {configuration_json: $configuration_json, revision: $revision}}' \
  >"${artifact_dir}/configuration-stale-request.json"
if varlinkctl --no-ask-password call \
  "unix:${socket_path}" \
  io.github.Gustav0ar.WireView.ApplyConfiguration \
  "$(cat "${artifact_dir}/configuration-stale-request.json")" \
  >"${artifact_dir}/configuration-stale.out" 2>&1; then
  echo "daemon accepted a stale bulk configuration revision" >&2
  exit 1
fi
grep -Eqi 'RevisionConflict|revision conflict' \
  "${artifact_dir}/configuration-stale.out"
./target/debug/wireview --socket "${socket_path}" config reload \
  >"${artifact_dir}/configuration-revision-race-reload.out"
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-after-revision-race.json"
diff -u "${artifact_dir}/configuration-original.json" \
  "${artifact_dir}/configuration-after-revision-race.json"
./target/debug/wireview --socket "${socket_path}" config get fan.mode \
  >"${artifact_dir}/configuration-item-get.out"
grep -q '^fan.mode = curve$' "${artifact_dir}/configuration-item-get.out"
./target/debug/wireview --socket "${socket_path}" config get fan.mode --json \
  >"${artifact_dir}/configuration-item-get.json"
jq -e '.key == "fan.mode" and .value == "curve"' \
  "${artifact_dir}/configuration-item-get.json" >/dev/null
./target/debug/wireview --socket "${socket_path}" config set \
  backlight_percent 73 \
  >"${artifact_dir}/configuration-item-applied.out"
grep -q '^Applied backlight_percent = 73 temporarily.$' \
  "${artifact_dir}/configuration-item-applied.out"
test "$(wc -l <"${artifact_dir}/configuration-item-applied.out")" -eq 1
./target/debug/wireview --socket "${socket_path}" config get backlight_percent --json \
  >"${artifact_dir}/configuration-item-applied.json"
jq -e '.key == "backlight_percent" and .value == 73' \
  "${artifact_dir}/configuration-item-applied.json" >/dev/null
./target/debug/wireview --socket "${socket_path}" config reload \
  >"${artifact_dir}/configuration-item-reload.out"
grep -q '^Reloaded permanently stored configuration.$' \
  "${artifact_dir}/configuration-item-reload.out"
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-item-reloaded.json"
diff -u "${artifact_dir}/configuration-original.json" \
  "${artifact_dir}/configuration-item-reloaded.json"
if ./target/debug/wireview --socket "${socket_path}" config set \
  backlight_percent 101 \
  >"${artifact_dir}/configuration-item-invalid-cli.out" 2>&1; then
  echo "CLI accepted an invalid individual configuration value" >&2
  exit 1
fi
grep -q '^invalid argument: backlight_percent must be between 0 and 100$' \
  "${artifact_dir}/configuration-item-invalid-cli.out"
if varlinkctl --no-ask-password call \
  "unix:${socket_path}" \
  io.github.Gustav0ar.WireView.SetConfigurationItem \
  '{"key":"backlight_percent","value":"101","persist":false,"confirm":false}' \
  >"${artifact_dir}/configuration-item-invalid.out" 2>&1; then
  echo "daemon accepted an invalid individual configuration value" >&2
  exit 1
fi
if varlinkctl --no-ask-password call \
  "unix:${socket_path}" \
  io.github.Gustav0ar.WireView.RebootDevice \
  '{"confirm":false}' \
  >"${artifact_dir}/reboot-without-confirmation.out" 2>&1; then
  echo "daemon accepted a reboot without API-boundary confirmation" >&2
  exit 1
fi
varlinkctl --no-ask-password call \
  "unix:${socket_path}" \
  io.github.Gustav0ar.WireView.GetConfigurationItem \
  '{"key":"fan.mode"}' \
  >"${artifact_dir}/configuration-item-varlink.json"
jq -e '.key == "fan.mode" and .value_json == "\"curve\""' \
  "${artifact_dir}/configuration-item-varlink.json" >/dev/null
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-after-invalid-item.json"
diff -u "${artifact_dir}/configuration-original.json" \
  "${artifact_dir}/configuration-after-invalid-item.json"
jq '.settings.backlight_percent = 101' \
  "${artifact_dir}/configuration-original.json" \
  >"${artifact_dir}/configuration-invalid.json"
invalid_configuration="$(jq -c .settings "${artifact_dir}/configuration-invalid.json")"
configuration_revision="$(jq -r .revision "${artifact_dir}/configuration-invalid.json")"
jq -cn --arg configuration_json "${invalid_configuration}" \
  --arg revision "${configuration_revision}" \
  '{configuration: {configuration_json: $configuration_json, revision: $revision}}' \
  >"${artifact_dir}/configuration-invalid-request.json"
if varlinkctl --no-ask-password call \
  "unix:${socket_path}" \
  io.github.Gustav0ar.WireView.ApplyConfiguration \
  "$(cat "${artifact_dir}/configuration-invalid-request.json")" \
  >"${artifact_dir}/configuration-invalid.out" 2>&1; then
  echo "daemon accepted invalid configuration through raw Varlink" >&2
  exit 1
fi
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-after-invalid.json"
diff -u "${artifact_dir}/configuration-original.json" \
  "${artifact_dir}/configuration-after-invalid.json"
jq '.settings.display.primary_color = "#FFFFFF"' \
  "${artifact_dir}/configuration-original.json" \
  >"${artifact_dir}/configuration-invalid-color.json"
invalid_color_configuration="$(
  jq -c .settings "${artifact_dir}/configuration-invalid-color.json"
)"
invalid_color_revision="$(jq -r .revision "${artifact_dir}/configuration-invalid-color.json")"
jq -cn --arg configuration_json "${invalid_color_configuration}" \
  --arg revision "${invalid_color_revision}" \
  '{configuration: {configuration_json: $configuration_json, revision: $revision}}' \
  >"${artifact_dir}/configuration-invalid-color-request.json"
if varlinkctl --no-ask-password call \
  "unix:${socket_path}" \
  io.github.Gustav0ar.WireView.ApplyConfiguration \
  "$(cat "${artifact_dir}/configuration-invalid-color-request.json")" \
  >"${artifact_dir}/configuration-invalid-color.out" 2>&1; then
  echo "daemon accepted malformed RGB color through raw Varlink" >&2
  exit 1
fi
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-after-invalid-color.json"
diff -u "${artifact_dir}/configuration-original.json" \
  "${artifact_dir}/configuration-after-invalid-color.json"
./target/debug/wireview --socket "${socket_path}" config show \
  >"${artifact_dir}/configuration.out"
jq '.settings.backlight_percent = 73
    | .settings.logging_interval_seconds = 30
    | .settings.display.highlight_color = "80E64121"' \
  "${artifact_dir}/configuration-original.json" \
  >"${artifact_dir}/configuration-edited.json"
./target/debug/wireview --socket "${socket_path}" config apply \
  "${artifact_dir}/configuration-edited.json" --json \
  >"${artifact_dir}/configuration-apply-result.json"
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-applied.json"
./target/debug/wireview --socket "${socket_path}" config reload --json \
  >"${artifact_dir}/configuration-reload-result.json"
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-reloaded.json"
./target/debug/wireview --socket "${socket_path}" config store \
  "${artifact_dir}/configuration-edited.json" --yes --json \
  >"${artifact_dir}/configuration-store-result.json"
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-stored.json"
./target/debug/wireview --socket "${socket_path}" config reload --json \
  >"${artifact_dir}/configuration-stored-reload-result.json"
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-stored-reload.json"
./target/debug/wireview --socket "${socket_path}" config reset --yes --json \
  >"${artifact_dir}/configuration-reset-result.json"
./target/debug/wireview --socket "${socket_path}" config show --json \
  >"${artifact_dir}/configuration-reset.json"
./target/debug/wireview --socket "${socket_path}" debug factory-reset --yes \
  >"${artifact_dir}/debug-factory-reset.out"
grep -q '^Factory defaults restored and stored permanently' \
  "${artifact_dir}/debug-factory-reset.out"
./target/debug/wireview --socket "${socket_path}" history --format csv \
  >"${artifact_dir}/history.csv"
./target/debug/wireview --socket "${socket_path}" history --format table \
  >"${artifact_dir}/history.out"
./target/debug/wireview --socket "${socket_path}" history --format json \
  --output "${artifact_dir}/history.json" \
  >"${artifact_dir}/history-write.out"
./target/debug/wireview --socket "${socket_path}" history --format raw \
  --output "${artifact_dir}/history.raw" \
  >"${artifact_dir}/history-raw-write.out"
./target/debug/wireview --socket "${socket_path}" debug monitor --count 4 \
  >"${artifact_dir}/events.jsonl" &
monitor_pid=$!

for screen in main current temp status simple same temperature; do
  ./target/debug/wireview --socket "${socket_path}" screen "${screen}" \
    >>"${artifact_dir}/screens.out"
done
wait "${monitor_pid}"
monitor_pid=""

./target/debug/wireview --socket "${socket_path}" debug monitor \
  >"${artifact_dir}/live-events.jsonl" &
monitor_pid=$!
live_event=0
for _ in {1..40}; do
  if [[ -s "${artifact_dir}/live-events.jsonl" ]]; then
    live_event=1
    break
  fi
  sleep 0.05
done
if [[ "${live_event}" -ne 1 ]]; then
  echo "monitor output was not flushed while streaming" >&2
  exit 1
fi
kill "${monitor_pid}"
wait "${monitor_pid}" 2>/dev/null || true
monitor_pid=""

./target/debug/wireview --socket "${socket_path}" telemetry --watch \
  >"${artifact_dir}/watch.out" &
monitor_pid=$!
watch_ready=0
for _ in {1..40}; do
  if [[ "$(grep -c 'Connection: Connected' "${artifact_dir}/watch.out" || true)" -ge 2 ]]; then
    watch_ready=1
    break
  fi
  sleep 0.05
done
if [[ "${watch_ready}" -ne 1 ]]; then
  echo "telemetry watch did not refresh in place" >&2
  exit 1
fi
kill -INT "${monitor_pid}"
wait "${monitor_pid}"
monitor_pid=""
cursor_up_pattern=$'\033\\[[0-9]+A'
grep -Eq "${cursor_up_pattern}" "${artifact_dir}/watch.out"
if grep -Fq $'\033[2J' "${artifact_dir}/watch.out" \
  || grep -Fq $'\033[?1049' "${artifact_dir}/watch.out"; then
  echo "telemetry watch cleared or replaced the terminal screen" >&2
  exit 1
fi

if ./target/debug/wireview --socket "${socket_path}" screen invalid \
  >"${artifact_dir}/invalid-screen.out" 2>&1; then
  echo "invalid screen command unexpectedly succeeded" >&2
  exit 1
fi

jq -e '
  .session_id == 1
  and (.sequence > 0)
  and (.observed_at_ms > 0)
  and (.pin_currents_a | length) == 6
  and (.pin_voltages_v | length) == 6
  and (.pin_power_w | length) == 6
  and (.vdd_v > 0)
  and (.avg_voltage_v > 0)
  and (.total_current_a > 0)
  and (.total_power_w > 0)
  and (.fan_duty_percent >= 0 and .fan_duty_percent <= 100)
  and (.cable_capability_w == 600)
  and (.stale == false)
' "${artifact_dir}/telemetry.json" >/dev/null
jq -e '
  .vendor_id == 239
  and .product_id == 5
  and .product_name == "WireView Pro II"
  and .build_string == "mock-build"
  and (.capabilities | index("telemetry") != null)
' "${artifact_dir}/device-info.json" >/dev/null
jq -e '
  .active_mask == 0
  and .logged_mask == 0
  and (.active | length) == 0
  and (.logged | length) == 0
' "${artifact_dir}/faults.json" >/dev/null
grep -Fxq 'Telemetry polling interval: 100 ms' "${artifact_dir}/poll-original.out"
grep -Fxq 'Telemetry polling interval: 250 ms' "${artifact_dir}/poll-updated.out"
grep -Fxq 'Telemetry polling interval: 100 ms' "${artifact_dir}/poll-restored.out"
jq -e '
  (.revision | length) > 0
  and .settings.friendly_name == "Mock WireView"
  and .settings.backlight_percent == 100
  and (.settings | has("raw_version") | not)
  and (.settings | has("crc") | not)
  and .settings.fan.mode == "curve"
  and .settings.fan.temperature_source == "maximum"
  and (.settings.fault_actions.display | length) == 6
  and .settings.fault_thresholds.wire_current_a == 10.5
  and .settings.averaging_ms == 1417
  and .settings.display.current_scale_a == 10
  and .settings.display.power_scale == "watts600"
  and .settings.display.default_screen == "main"
  and .settings.display.primary_color == "FFFFFF"
  and .settings.display.secondary_color == "646464"
  and .settings.display.highlight_color == "E64121"
  and .settings.display.background_color == "000000"
' "${artifact_dir}/configuration-original.json" >/dev/null
jq -e '
  .settings.backlight_percent == 73
  and .settings.logging_interval_seconds == 30
  and .settings.display.highlight_color == "80E64121"
' "${artifact_dir}/configuration-applied.json" >/dev/null
for result in configuration-apply-result configuration-reload-result \
  configuration-store-result configuration-stored-reload-result \
  configuration-reset-result; do
  jq -e '.success == true and (.message | length > 0)' \
    "${artifact_dir}/${result}.json" >/dev/null
done
diff -u "${artifact_dir}/configuration-original.json" \
  "${artifact_dir}/configuration-reloaded.json"
jq -S .settings "${artifact_dir}/configuration-edited.json" \
  >"${artifact_dir}/configuration-edited-settings.json"
jq -S .settings "${artifact_dir}/configuration-stored.json" \
  >"${artifact_dir}/configuration-stored-settings.json"
jq -S .settings "${artifact_dir}/configuration-stored-reload.json" \
  >"${artifact_dir}/configuration-stored-reload-settings.json"
diff -u "${artifact_dir}/configuration-edited-settings.json" \
  "${artifact_dir}/configuration-stored-settings.json"
diff -u "${artifact_dir}/configuration-edited-settings.json" \
  "${artifact_dir}/configuration-stored-reload-settings.json"
diff -u "${artifact_dir}/configuration-original.json" \
  "${artifact_dir}/configuration-reset.json"
grep -q '^state=ready ' "${artifact_dir}/status.out"
grep -Fq 'Primary / secondary colors: FFFFFF / 646464' \
  "${artifact_dir}/configuration.out"
grep -Fq 'Highlight / background colors: E64121 / 000000' \
  "${artifact_dir}/configuration.out"
for label in 'Connection: Connected' 'Last updated:' 'Average voltage:' 'Total current:' \
  'Total power:' 'Internal supply (VDD):' 'Cable power rating:' 'Fan duty:' \
  'Connector pins' 'Pin 1:' 'Pin 6:' 'Temperatures' 'Onboard input:' \
  'Onboard output:' 'External sensor 1:' 'External sensor 2:' 'Faults' \
  'Active:' 'Logged:'; do
  grep -Fq "${label}" "${artifact_dir}/telemetry.out"
done
diff -u <(printf '%s\n' Main Current Temp Status Simple Same Temp) \
  "${artifact_dir}/screens.out"
grep -Fxq 'invalid argument: unknown screen "invalid"' \
  "${artifact_dir}/invalid-screen.out"
grep -q '^device_time_ms,event,total_power_w' "${artifact_dir}/history.csv"
[[ "$(wc -l <"${artifact_dir}/history.csv")" -eq 2 ]]
grep -Fq 'Device time  Event' "${artifact_dir}/history.out"
grep -Fq '00:00:00.042' "${artifact_dir}/history.out"
grep -Fq '1:12.1/0.3' "${artifact_dir}/history.out"
jq -e '
  length == 1
  and .[0].kind == "measurement"
  and .[0].device_time_ms == 42
  and (.[0].metrics.pins | length) == 6
' "${artifact_dir}/history.json" >/dev/null
[[ "$(stat -c %s "${artifact_dir}/history.raw")" -eq 8388608 ]]

grep -q 'Vendor: wireviewd contributors' "${artifact_dir}/varlink-info.out"
jq -e '
  .state == "ready"
  and .session_id == 1
  and .api_version == 1
  and (.api_compatibility_id | test("^wireview-1-[0-9a-f]{16}$"))
  and (.api_capabilities | index("history-dump") != null)
  and (.api_capabilities | index("configuration-items") != null)
  and (.daemon_version | length) > 0
  and (.daemon_build_id | length) > 0
' \
  "${artifact_dir}/varlink-status.json" >/dev/null

jq -s -e '
  length == 4
  and all(.[]; .session_id == 1 and .sequence > 0)
  and any(.[]; .event == "screen_changed" or .event == "telemetry_updated")
' "${artifact_dir}/events.jsonl" >/dev/null

stop_daemon
trap - EXIT
rm -rf -- "${artifact_dir}"

echo "Varlink comprehensive smoke test passed"
