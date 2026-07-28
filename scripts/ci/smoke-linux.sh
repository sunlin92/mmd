#!/usr/bin/env bash
set -euo pipefail

artifact_dir=${1:?artifact directory required}
artifact_dir=$(CDPATH= cd -- "$artifact_dir" && pwd -P)
[[ "$(uname -m)" == 'x86_64' ]]
node scripts/ci/artifact-manifest.mjs verify "$artifact_dir"

appimage=$(find "$artifact_dir" -maxdepth 1 -type f -name '*.AppImage' -print)
deb=$(find "$artifact_dir" -maxdepth 1 -type f -name '*.deb' -print)
[[ $(printf '%s\n' "$appimage" | sed '/^$/d' | wc -l | tr -d ' ') == 1 ]]
[[ $(printf '%s\n' "$deb" | sed '/^$/d' | wc -l | tr -d ' ') == 1 ]]
dpkg-deb --info "$deb"
sudo apt-get update
sudo apt-get install -y "$deb"

run_for_five_seconds() {
  local log=$1
  shift
  "$@" >"$log" 2>&1 &
  local pid=$!
  sleep 5
  kill -0 "$pid"
  kill "$pid"
  wait "$pid" || true
}

installed_binary=$(command -v mmd)
run_for_five_seconds "$RUNNER_TEMP/mmd-deb.log" xvfb-run -a "$installed_binary"

[[ -c /dev/fuse ]]
ldconfig -p | grep -q 'libfuse\.so\.2'
chmod +x "$appimage"
run_for_five_seconds "$RUNNER_TEMP/mmd-appimage.log" xvfb-run -a "$appimage"
