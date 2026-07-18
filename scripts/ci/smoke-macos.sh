#!/usr/bin/env bash
set -euo pipefail

artifact_dir=${1:?artifact directory required}
expected_arch=${2:?expected architecture required}
[[ "$(uname -m)" == "$expected_arch" ]]
node scripts/ci/artifact-manifest.mjs verify "$artifact_dir"

mapfile_name="$RUNNER_TEMP/mmd-hdiutil.txt"
mount_point=
app_pid=
cleanup() {
  if [[ -n "$app_pid" ]] && kill -0 "$app_pid" 2>/dev/null; then kill "$app_pid" || true; fi
  if [[ -n "$mount_point" ]]; then hdiutil detach "$mount_point" -force || true; fi
}
trap cleanup EXIT

dmg=$(find "$artifact_dir" -maxdepth 1 -type f -name '*.dmg' -print)
[[ $(printf '%s\n' "$dmg" | sed '/^$/d' | wc -l | tr -d ' ') == 1 ]]
hdiutil attach -nobrowse -readonly "$dmg" | tee "$mapfile_name"
mount_point=$(awk '/\/Volumes\// { print substr($0, index($0, "/Volumes/")); exit }' "$mapfile_name")
[[ -n "$mount_point" ]]

app=$(find "$mount_point" -maxdepth 2 -type d -name 'MMD.app' -print)
[[ $(printf '%s\n' "$app" | sed '/^$/d' | wc -l | tr -d ' ') == 1 ]]
binary=$(find "$app/Contents/MacOS" -maxdepth 1 -type f -perm -111 -print)
[[ $(printf '%s\n' "$binary" | sed '/^$/d' | wc -l | tr -d ' ') == 1 ]]
[[ "$(lipo -archs "$binary")" == "$expected_arch" ]]
codesign --verify --deep --strict --verbose=2 "$app"
signature=$(codesign -dv --verbose=4 "$app" 2>&1 || true)
if grep -q 'Authority=Developer ID Application' <<<"$signature"; then
  echo 'macOS signing classification: Developer ID Application'
elif grep -q 'Signature=adhoc' <<<"$signature"; then
  echo 'macOS signing classification: ad-hoc'
else
  echo "$signature"
  echo 'Unrecognized macOS signing classification.' >&2
  exit 1
fi

"$binary" >"$RUNNER_TEMP/mmd-macos.log" 2>&1 &
app_pid=$!
sleep 5
kill -0 "$app_pid"
kill "$app_pid"
wait "$app_pid" || true
app_pid=
