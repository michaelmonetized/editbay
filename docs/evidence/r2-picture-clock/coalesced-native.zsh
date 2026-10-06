#!/usr/bin/env zsh
set -uo pipefail
cd /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker || exit 1
export TMPDIR="$PWD/artifacts/tmp"
unset LD_LIBRARY_PATH
dst=artifacts/picture-clock/coalesced-trials
mkdir "$dst" || exit 1
lab="$PWD/artifacts/picture-clock/coalesced-bin/editbay-lab"
app="$PWD/artifacts/picture-clock/coalesced-bin/editbay-studio"
native_failures=0
run_native() {
  local trial_name="$1"
  shift
  if "$@" > "$dst/$trial_name.json" 2> "$dst/$trial_name.log"; then
    printf '%s\t0\n' "$trial_name" >> "$dst/exits.tsv"
  else
    local trial_exit=$?
    printf '%s\t%s\n' "$trial_name" "$trial_exit" >> "$dst/exits.tsv"
    (( native_failures += 1 ))
  fi
}
date -u +%FT%TZ > "$dst/started.txt"
cat /proc/meminfo > "$dst/host-memory-before.txt"
cat /proc/pressure/memory > "$dst/host-pressure-before.txt"
for fixture in camera six; do
  if [[ "$fixture" == camera ]]; then
    media='/home/michael/Downloads/iCloud Photos/IMG_0001.MP4'
    project=artifacts/streaming/qualified-camera/Preview.editbay
  else
    media=artifacts/natural-sound/fixtures/qualified-six-channel.mov
    project=artifacts/streaming/qualified-six-channel/Preview.editbay
  fi
  run_native "continuous-$fixture" "$lab" native-continuous "$app" "$project" "$dst/continuous-$fixture"
  run_native "prepared-$fixture" "$lab" native-prepared "$app" "$project" "$dst/prepared-$fixture"
  run_native "workflow-$fixture" "$lab" native-playback "$app" "$media" "$dst/workflow-$fixture" full
  run_native "long-$fixture" "$lab" native-long "$app" "artifacts/sound-index/qualified-$fixture/Long.editbay" "$dst/long-$fixture"
done
cat /proc/meminfo > "$dst/host-memory-after.txt"
cat /proc/pressure/memory > "$dst/host-pressure-after.txt"
date -u +%FT%TZ > "$dst/finished.txt"
exit "$native_failures"
