#!/usr/bin/env zsh
set -euo pipefail
cd /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
export TMPDIR="$PWD/artifacts/tmp"
unset LD_LIBRARY_PATH
dst=artifacts/picture-clock
lab="$PWD/$dst/qualified-bin/editbay-lab"
app="$PWD/$dst/qualified-bin/editbay-studio"
mkdir "$dst/qualified-trials"
date -u +%FT%TZ > "$dst/qualified-trials/started.txt"
cat /proc/meminfo > "$dst/qualified-trials/host-memory-before.txt"
cat /proc/pressure/memory > "$dst/qualified-trials/host-pressure-before.txt"
for fixture in camera six; do
  if [[ "$fixture" == camera ]]; then
    media='/home/michael/Downloads/iCloud Photos/IMG_0001.MP4'
    project=artifacts/streaming/qualified-camera/Preview.editbay
  else
    media=artifacts/natural-sound/fixtures/qualified-six-channel.mov
    project=artifacts/streaming/qualified-six-channel/Preview.editbay
  fi
  "$lab" native-continuous "$app" "$project" "$dst/qualified-trials/continuous-$fixture" > "$dst/qualified-trials/continuous-$fixture.json" 2> "$dst/qualified-trials/continuous-$fixture.log"
  "$lab" native-prepared "$app" "$project" "$dst/qualified-trials/prepared-$fixture" > "$dst/qualified-trials/prepared-$fixture.json" 2> "$dst/qualified-trials/prepared-$fixture.log"
  "$lab" native-playback "$app" "$media" "$dst/qualified-trials/workflow-$fixture" full > "$dst/qualified-trials/workflow-$fixture.json" 2> "$dst/qualified-trials/workflow-$fixture.log"
  "$lab" native-long "$app" "artifacts/sound-index/qualified-$fixture/Long.editbay" "$dst/qualified-trials/long-$fixture" > "$dst/qualified-trials/long-$fixture.json" 2> "$dst/qualified-trials/long-$fixture.log"
done
cat /proc/meminfo > "$dst/qualified-trials/host-memory-after.txt"
cat /proc/pressure/memory > "$dst/qualified-trials/host-pressure-after.txt"
date -u +%FT%TZ > "$dst/qualified-trials/finished.txt"
