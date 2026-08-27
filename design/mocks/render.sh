#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "render.sh needs an X11 or Wayland display" >&2
    exit 1
fi

mock_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
render_dir="$mock_dir/rendered"
mkdir -p "$render_dir"

designs=(copper-bus bench-console conductor-field)
states=(fault ready stale)

for design in "${designs[@]}"; do
    slint-viewer --check "$mock_dir/$design.slint"
    for state in "${states[@]}"; do
        slint-viewer "$mock_dir/$design.slint" \
            --load-data "$mock_dir/fixtures/$state.json" \
            --screenshot "$render_dir/$design-$state.png" \
            --backend winit-software
    done
done
