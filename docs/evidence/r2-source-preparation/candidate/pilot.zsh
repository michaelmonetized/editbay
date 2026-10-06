#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
export TMPDIR="$root/artifacts/tmp"
unset LD_LIBRARY_PATH
base="$root/artifacts/source-preparation/candidate/pilot"
lab="$root/artifacts/source-preparation/candidate/bin/editbay-lab"
mkdir -p "$base"
run_case() {
  local case_name="$1"
  shift
  local began="$(date -u +%FT%TZ)"
  "$@" > "$base/$case_name.json" 2> "$base/$case_name.log"
  local result_code=$?
  printf '%s\t%s\t%s\t%s\n' "$case_name" "$began" "$(date -u +%FT%TZ)" "$result_code" >> "$base/exits.tsv"
}
run_case camera "$lab" sustain-sound '/home/michael/Downloads/iCloud Photos/IMG_0001.MP4' 6 "$base/camera"
run_case six "$lab" sustain-sound "$root/artifacts/natural-sound/fixtures/qualified-six-channel.mov" 6 "$base/six"
run_case quiet "$lab" sustain-sound "$root/artifacts/streaming/fixtures/quiet-45s.mov" 180 "$base/quiet"
run_case resume-device "$lab" stream-sound "$root/artifacts/streaming/fixtures/quiet-45s.mov" resume-device
