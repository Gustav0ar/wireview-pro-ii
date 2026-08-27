#!/usr/bin/env bash
set -Eeuo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin_dir="${WIREVIEW_SMOKE_BIN_DIR:-${project_dir}/target/debug}"
artifact_dir="$(mktemp -d)"
socket_path="${artifact_dir}/wireview.varlink"
xvfb_pid=""
daemon_pid=""

report_error() {
  local status="$1"
  local line="$2"
  local command="$3"
  trap - ERR
  printf 'Desktop smoke test failed at line %s: %s\n' \
    "${line}" "${command}" >&2
  for log in "${artifact_dir}"/*.log; do
    if [[ -f "${log}" ]]; then
      printf '%s\n' "Log: ${log}" >&2
      tail -n 80 "${log}" >&2
    fi
  done
  exit "${status}"
}

cleanup() {
  if [[ -n "${daemon_pid}" ]]; then
    kill -TERM "${daemon_pid}" 2>/dev/null || true
    wait "${daemon_pid}" 2>/dev/null || true
  fi
  if [[ -n "${xvfb_pid}" ]]; then
    kill -TERM "${xvfb_pid}" 2>/dev/null || true
    wait "${xvfb_pid}" 2>/dev/null || true
  fi
  rm -rf -- "${artifact_dir}"
}
trap 'report_error "$?" "${LINENO}" "${BASH_COMMAND}"' ERR
trap cleanup EXIT

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "required command not found: $1" >&2
    exit 1
  fi
}

require_command python3
require_command timeout
if [[ "${WIREVIEW_SKIP_BUILD:-}" != "1" ]]; then
  require_command cargo
fi

xvfb="${WIREVIEW_XVFB:-}"
if [[ -z "${xvfb}" ]]; then
  xvfb="$(command -v Xvfb || true)"
fi
if [[ -z "${xvfb}" || ! -x "${xvfb}" ]]; then
  echo "required command not found: Xvfb (or set WIREVIEW_XVFB)" >&2
  exit 1
fi

display_number=""
for candidate in {90..119}; do
  if [[ ! -e "/tmp/.X11-unix/X${candidate}" && \
        ! -e "/tmp/.X${candidate}-lock" ]]; then
    display_number="${candidate}"
    break
  fi
done
if [[ -z "${display_number}" ]]; then
  echo "no free X display found from :90 through :119" >&2
  exit 1
fi
display=":${display_number}"

"${xvfb}" "${display}" \
  -screen 0 1440x900x24 \
  -nolisten tcp \
  -ac \
  >"${artifact_dir}/xvfb.log" 2>&1 &
xvfb_pid=$!
for _ in {1..50}; do
  if [[ -S "/tmp/.X11-unix/X${display_number}" ]]; then
    break
  fi
  if ! kill -0 "${xvfb_pid}" 2>/dev/null; then
    echo "Xvfb exited before creating ${display}" >&2
    exit 1
  fi
  sleep 0.05
done
test -S "/tmp/.X11-unix/X${display_number}"

cd "${project_dir}"
if [[ "${WIREVIEW_SKIP_BUILD:-}" != "1" ]]; then
  cargo build --workspace --bins --locked
fi
for binary in wireviewd wireview wireview-gui; do
  if [[ ! -x "${bin_dir}/${binary}" ]]; then
    echo "missing smoke-test binary: ${bin_dir}/${binary}" >&2
    exit 1
  fi
done

run_window() {
  local name="$1"
  shift
  local status=0
  env -u WAYLAND_DISPLAY \
    DISPLAY="${display}" \
    SLINT_BACKEND=winit-software \
    timeout --signal=TERM 1s \
    "${bin_dir}/wireview-gui" --no-tray "$@" \
    >"${artifact_dir}/${name}.log" 2>&1 || status=$?
  if [[ "${status}" -ne 124 ]]; then
    echo "wireview-gui ${name} exited with status ${status}, expected 124" >&2
    return 1
  fi
  if grep -Eiq 'panic|wireview-gui: failed' "${artifact_dir}/${name}.log"; then
    echo "wireview-gui ${name} reported a runtime failure" >&2
    return 1
  fi
}

run_minimum_window() {
  local page="$1"
  local status=0
  env -u WAYLAND_DISPLAY \
    DISPLAY="${display}" \
    SLINT_BACKEND=winit-software \
    timeout --signal=TERM 1s \
    "${bin_dir}/wireview-gui" --no-tray --demo ready --page "${page}" \
    >"${artifact_dir}/minimum-${page}.log" 2>&1 &
  local runner_pid=$!

  DISPLAY="${display}" python3 - <<'PY'
import ctypes
import time
from ctypes import POINTER, byref, c_char_p, c_int, c_uint, c_ulong, c_void_p

x11 = ctypes.CDLL("libX11.so.6")
x11.XOpenDisplay.argtypes = [c_char_p]
x11.XOpenDisplay.restype = c_void_p
x11.XDefaultRootWindow.argtypes = [c_void_p]
x11.XDefaultRootWindow.restype = c_ulong
x11.XQueryTree.argtypes = [
    c_void_p,
    c_ulong,
    POINTER(c_ulong),
    POINTER(c_ulong),
    POINTER(POINTER(c_ulong)),
    POINTER(c_uint),
]
x11.XQueryTree.restype = c_int
x11.XFetchName.argtypes = [c_void_p, c_ulong, POINTER(c_char_p)]
x11.XFetchName.restype = c_int
x11.XGetClassHint.argtypes = [c_void_p, c_ulong, c_void_p]
x11.XGetClassHint.restype = c_int
x11.XResizeWindow.argtypes = [c_void_p, c_ulong, c_uint, c_uint]
x11.XResizeWindow.restype = c_int
x11.XFlush.argtypes = [c_void_p]
x11.XFlush.restype = c_int
x11.XFree.argtypes = [c_void_p]
x11.XFree.restype = c_int
x11.XCloseDisplay.argtypes = [c_void_p]
x11.XCloseDisplay.restype = c_int


class XClassHint(ctypes.Structure):
    _fields_ = [("res_name", c_void_p), ("res_class", c_void_p)]

display = x11.XOpenDisplay(None)
if not display:
    raise SystemExit("failed to open the Xvfb display")


def find_window(window):
    name = c_char_p()
    if x11.XFetchName(display, window, byref(name)) and name.value:
        title = name.value.decode(errors="replace")
        x11.XFree(name)
        if title == "WireView Pro II":
            return window

    root = c_ulong()
    parent = c_ulong()
    children = POINTER(c_ulong)()
    count = c_uint()
    if not x11.XQueryTree(
        display,
        window,
        byref(root),
        byref(parent),
        byref(children),
        byref(count),
    ):
        return None
    values = [children[index] for index in range(count.value)]
    if children:
        x11.XFree(children)
    for child in values:
        found = find_window(child)
        if found:
            return found
    return None


window = None
for _ in range(40):
    window = find_window(x11.XDefaultRootWindow(display))
    if window:
        break
    time.sleep(0.01)
if not window:
    raise SystemExit("WireView window did not appear")
class_hint = XClassHint()
if not x11.XGetClassHint(display, window, byref(class_hint)):
    raise SystemExit("WireView window did not publish WM_CLASS")
class_name = ctypes.string_at(class_hint.res_class).decode(errors="replace")
if class_hint.res_name:
    x11.XFree(class_hint.res_name)
if class_hint.res_class:
    x11.XFree(class_hint.res_class)
if class_name != "io.github.Gustav0ar.WireView":
    raise SystemExit(f"unexpected WireView WM_CLASS: {class_name!r}")
if not x11.XResizeWindow(display, window, 1120, 720):
    raise SystemExit("failed to resize the WireView window")
x11.XFlush(display)
x11.XCloseDisplay(display)
PY

  wait "${runner_pid}" || status=$?
  if [[ "${status}" -ne 124 ]]; then
    echo "wireview-gui minimum-${page} exited with status ${status}, expected 124" >&2
    return 1
  fi
  if grep -Eiq 'panic|wireview-gui: failed' \
    "${artifact_dir}/minimum-${page}.log"; then
    echo "wireview-gui minimum-${page} reported a runtime failure" >&2
    return 1
  fi
}

for page in overview pins graphs faults history configure themes device; do
  run_window "demo-${page}" --demo ready --page "${page}"
done
run_window demo-fault --demo fault --page faults
run_window demo-stale --demo stale --page pins
run_window demo-offline --demo offline --page device
for page in overview pins graphs faults history configure themes device; do
  run_minimum_window "${page}"
done

"${bin_dir}/wireviewd" \
  --mock \
  --socket "${socket_path}" \
  --poll-ms 100 \
  >"${artifact_dir}/daemon.log" 2>&1 &
daemon_pid=$!

daemon_ready=0
for _ in {1..50}; do
  if "${bin_dir}/wireview" --socket "${socket_path}" status \
    >"${artifact_dir}/status.log" 2>&1; then
    daemon_ready=1
    break
  fi
  if ! kill -0 "${daemon_pid}" 2>/dev/null; then
    echo "mock daemon exited before becoming ready" >&2
    exit 1
  fi
  sleep 0.1
done
test "${daemon_ready}" -eq 1
grep -Fq 'state=ready' "${artifact_dir}/status.log"
grep -Fq 'api=2' "${artifact_dir}/status.log"

run_window mock-daemon --socket "${socket_path}" --page overview
"${bin_dir}/wireview" --socket "${socket_path}" telemetry --json \
  >"${artifact_dir}/telemetry.json"
grep -Fq '"pin_currents_a"' "${artifact_dir}/telemetry.json"

echo "Desktop smoke test passed"
