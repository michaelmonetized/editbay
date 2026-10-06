#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
export TMPDIR="$root/artifacts/tmp"
unset LD_LIBRARY_PATH
base="$root/artifacts/recovery-publication/observed"
lab="$base/bin/editbay-lab"
app="$base/bin/editbay-studio"
run_case() {
  local case_name="$1"
  shift
  local began="$(date -u +%FT%TZ)"
  "$@" > "$base/$case_name.json" 2> "$base/$case_name.log"
  local result_code=$?
  printf '%s\t%s\t%s\t%s\n' "$case_name" "$began" "$(date -u +%FT%TZ)" "$result_code" >> "$base/exits.tsv"
}
run_case camera-range-jobs "$lab" native-range-jobs "$app" artifacts/streaming/qualified-camera/Preview.editbay artifacts/pcm-canonical/candidate/ranges-camera/Full-48000.mov "$base/camera-range-jobs"
run_case six-channel-range-jobs "$lab" native-range-jobs "$app" artifacts/streaming/qualified-six-channel/Preview.editbay artifacts/pcm-canonical/candidate/ranges-six-channel-canonical/Full-48000.mov "$base/six-channel-range-jobs"
run_case timing "$lab" native-workspace-timing "$app" "$base/timing"
run_case errors "$lab" native-workspace-errors "$app" "$base/errors"
run_case workspace "$lab" native-workspace "$app" "$base/workspace" 100
zsh artifacts/recovery-publication/observed-checks.zsh
