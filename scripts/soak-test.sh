#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
duration="${WIREVIEW_SOAK_SECONDS:-86400}"
interval="${WIREVIEW_SOAK_INTERVAL_SECONDS:-60}"
max_failures="${WIREVIEW_SOAK_MAX_FAILURES:-0}"
max_rss_growth_kib="${WIREVIEW_SOAK_MAX_RSS_GROWTH_KIB:-32768}"
max_event_lag="${WIREVIEW_SOAK_MAX_EVENT_LAG:-0}"
wireview_bin="${WIREVIEW_SOAK_BIN:-wireview}"
service="${WIREVIEW_SOAK_SERVICE:-wireviewd.service}"
timestamp="$(date --utc '+%Y%m%dT%H%M%SZ')"
artifact_dir="${WIREVIEW_SOAK_DIR:-}"
if [[ -z "${artifact_dir}" ]]; then
  artifact_dir="${project_dir}/target/soak-${timestamp}"
fi

for value_name in duration interval max_failures max_rss_growth_kib max_event_lag; do
  value="${!value_name}"
  if [[ ! "${value}" =~ ^[0-9]+$ ]]; then
    echo "${value_name} must be a non-negative integer" >&2
    exit 2
  fi
done
if (( duration == 0 || interval == 0 )); then
  echo "duration and interval must be greater than zero" >&2
  exit 2
fi
if [[ -z "${artifact_dir}" || "${artifact_dir}" == "/" ]]; then
  echo "unsafe soak artifact directory" >&2
  exit 2
fi
for command in "${wireview_bin}" systemctl jq ps awk journalctl; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    echo "required command not found: ${command}" >&2
    exit 1
  fi
done

install -d "${artifact_dir}"
samples_csv="${artifact_dir}/samples.csv"
errors_log="${artifact_dir}/errors.log"
summary_file="${artifact_dir}/summary.txt"
printf '%s\n' \
  'observed_at,pid,session,sequence,device_observed_ms,rss_kib,cpu_percent,result' \
  >"${samples_csv}"
: >"${errors_log}"

start_epoch="$(date +%s)"
deadline=$((start_epoch + duration))
failures=0
session_changes=0
process_changes=0
last_session=""
last_pid=""
baseline_rss=-1
max_rss=0
stop_requested=0

stop_soak() {
  stop_requested=1
}
trap stop_soak INT TERM

while (( $(date +%s) <= deadline && stop_requested == 0 )); do
  observed_at="$(date --utc --iso-8601=seconds)"
  telemetry=""
  result="ok"
  if ! telemetry="$("${wireview_bin}" telemetry --json 2>>"${errors_log}")"; then
    result="telemetry-error"
  elif ! jq -e '
    (.stale == false)
    and (.sequence | type == "number")
    and (.session_id | type == "number")
    and (.observed_at_ms | type == "number")
  ' <<<"${telemetry}" >/dev/null; then
    result="stale-or-invalid"
  fi

  pid="$(systemctl show --property MainPID --value "${service}" 2>>"${errors_log}" || true)"
  if [[ ! "${pid}" =~ ^[1-9][0-9]*$ || ! -r "/proc/${pid}/status" ]]; then
    result="daemon-not-running"
    pid=""
  fi

  session=""
  sequence=""
  device_observed_ms=""
  rss_kib=""
  cpu_percent=""
  if [[ "${result}" == "ok" ]]; then
    session="$(jq -r '.session_id' <<<"${telemetry}")"
    sequence="$(jq -r '.sequence' <<<"${telemetry}")"
    device_observed_ms="$(jq -r '.observed_at_ms' <<<"${telemetry}")"
    rss_kib="$(
      awk '/^VmRSS:/ { print $2; exit }' "/proc/${pid}/status"
    )"
    cpu_percent="$(ps -p "${pid}" -o %cpu= | awk '{ print $1 }')"
    if [[ ! "${rss_kib}" =~ ^[0-9]+$ || -z "${cpu_percent}" ]]; then
      result="process-metrics-error"
    fi
  fi

  if [[ "${result}" == "ok" ]]; then
    if (( baseline_rss < 0 )); then
      baseline_rss="${rss_kib}"
    fi
    if (( rss_kib > max_rss )); then
      max_rss="${rss_kib}"
    fi
    if [[ -n "${last_session}" && "${session}" != "${last_session}" ]]; then
      session_changes=$((session_changes + 1))
    fi
    if [[ -n "${last_pid}" && "${pid}" != "${last_pid}" ]]; then
      process_changes=$((process_changes + 1))
    fi
    last_session="${session}"
    last_pid="${pid}"
  else
    failures=$((failures + 1))
    printf '%s: %s\n' "${observed_at}" "${result}" >>"${errors_log}"
  fi

  printf '%s,%s,%s,%s,%s,%s,%s,%s\n' \
    "${observed_at}" "${pid}" "${session}" "${sequence}" \
    "${device_observed_ms}" "${rss_kib}" "${cpu_percent}" "${result}" \
    >>"${samples_csv}"
  printf 'Soak: %ss/%ss, failures=%s, session changes=%s, RSS=%s KiB\r' \
    "$(( $(date +%s) - start_epoch ))" "${duration}" "${failures}" \
    "${session_changes}" "${rss_kib:-unavailable}"

  remaining=$((deadline - $(date +%s)))
  if (( remaining <= 0 || stop_requested != 0 )); then
    break
  fi
  sleep_for="${interval}"
  if (( sleep_for > remaining )); then
    sleep_for="${remaining}"
  fi
  sleep "${sleep_for}" || true
done
printf '\n'

sample_count="$(awk -F, 'NR > 1 && $8 == "ok" { count++ } END { print count + 0 }' "${samples_csv}")"
average_cpu="$(
  awk -F, '
    NR > 1 && $8 == "ok" { total += $7; count++ }
    END { if (count) printf "%.2f", total / count; else print "unavailable" }
  ' "${samples_csv}"
)"
rss_growth=0
if (( baseline_rss >= 0 )); then
  rss_growth=$((max_rss - baseline_rss))
fi
event_lag_count="$(
  journalctl -u "${service}" --since "@${start_epoch}" --no-pager -q \
    2>>"${errors_log}" \
    | awk '/Varlink event publisher lagged/ { count++ } END { print count + 0 }'
)"

{
  printf 'Started: %s\n' "$(date --utc --date="@${start_epoch}" --iso-8601=seconds)"
  printf 'Requested duration: %s seconds\n' "${duration}"
  printf 'Successful samples: %s\n' "${sample_count}"
  printf 'Failures: %s (limit %s)\n' "${failures}" "${max_failures}"
  printf 'Session changes: %s\n' "${session_changes}"
  printf 'Daemon process changes: %s\n' "${process_changes}"
  printf 'Baseline RSS: %s KiB\n' "${baseline_rss}"
  printf 'Maximum RSS: %s KiB\n' "${max_rss}"
  printf 'RSS growth: %s KiB (limit %s KiB)\n' \
    "${rss_growth}" "${max_rss_growth_kib}"
  printf 'Average CPU: %s%%\n' "${average_cpu}"
  printf 'Varlink lag events: %s (limit %s)\n' \
    "${event_lag_count}" "${max_event_lag}"
} | tee "${summary_file}"

if (( stop_requested != 0 )); then
  echo "Soak interrupted. Evidence: ${artifact_dir}" >&2
  exit 130
fi
if (( sample_count == 0
      || failures > max_failures
      || rss_growth > max_rss_growth_kib
      || event_lag_count > max_event_lag )); then
  echo "Soak qualification failed. Evidence: ${artifact_dir}" >&2
  exit 1
fi

echo "Soak qualification passed. Evidence: ${artifact_dir}"
