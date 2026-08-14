#!/usr/bin/env bash
# Run tests/persist.qml against a stand-in shell.
#
# Needs Quickshell and an Omarchy shell checkout, so it cannot run in CI — the
# `qs.Ui` and `qs.Commons` imports only exist on an Omarchy machine.
#
# It must run under the real Wayland session rather than offscreen: the
# offscreen platform has no layer-shell backend, so Quickshell refuses to build
# the panel types Service.qml pulls in.
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
plugin_dir="$(dirname -- "$here")"
shell_dir="${OMARCHY_PATH:-$HOME/.local/share/omarchy}/shell"

command -v qs >/dev/null 2>&1 || { echo "persist.sh: quickshell (qs) not found" >&2; exit 1; }
[[ -d $shell_dir/Ui ]] || { echo "persist.sh: no Omarchy shell at $shell_dir" >&2; exit 1; }
[[ -n ${WAYLAND_DISPLAY:-} ]] || { echo "persist.sh: needs a Wayland session" >&2; exit 1; }

work="$(mktemp -d)"
# shellcheck disable=SC2064  # expand now: this exact directory is the one to remove
trap "rm -rf '$work'" EXIT

mkdir -p "$work/plugin" /tmp/omarchy-pipelines-nobin
# The symlinks live here, in a temp dir — never inside the plugin folder, which
# the shell refuses to load if it contains any.
ln -s "$shell_dir/Ui" "$work/Ui"
ln -s "$shell_dir/Commons" "$work/Commons"
cp "$plugin_dir/Service.qml" "$plugin_dir/Panel.qml" "$plugin_dir/Model.js" "$work/plugin/"
cp "$here/persist.qml" "$work/shell.qml"

output="$(cd "$work" && timeout 40 qs -p "$work" 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | sed 's/^.*qml: //')"
echo "$output" | grep -E "^\s*(ok|FAIL)|persistence|assertions|FAILED" || true

if echo "$output" | grep -q "all assertions passed"; then
  exit 0
fi
echo "persist.sh: FAILED" >&2
exit 1
