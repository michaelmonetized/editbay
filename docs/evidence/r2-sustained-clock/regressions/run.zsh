#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
export TMPDIR="$root/artifacts/tmp"
unset LD_LIBRARY_PATH
lab="$root/artifacts/sustained-clock/buffer-bin/editbay-lab"
app="$root/artifacts/sustained-clock/buffer-bin/editbay-studio"
mkdir -p artifacts/sustained-clock/buffer-regressions
for fixture in camera six; do
  case "$fixture" in
    camera) project="$root/artifacts/streaming/qualified-camera/Preview.editbay";;
    six) project="$root/artifacts/streaming/qualified-six-channel/Preview.editbay";;
  esac
  began=$(date -u +%FT%TZ)
  "$lab" native-prepared "$app" "$project" "$root/artifacts/sustained-clock/buffer-regressions/$fixture" > "$root/artifacts/sustained-clock/buffer-regressions/$fixture.json" 2> "$root/artifacts/sustained-clock/buffer-regressions/$fixture.log"
  result=$?
  printf '%s\t%s\t%s\t%d\n' "$fixture" "$began" "$(date -u +%FT%TZ)" "$result" >> "$root/artifacts/sustained-clock/buffer-regressions/exits.tsv"
done
for mode in cancel kill underrun kill-device stall-device cancel-stalled-device prepare-cancel; do
  began=$(date -u +%FT%TZ)
  "$lab" stream-sound "$root/artifacts/streaming/fixtures/quiet-45s.mov" "$mode" > "$root/artifacts/sustained-clock/buffer-regressions/$mode.json" 2> "$root/artifacts/sustained-clock/buffer-regressions/$mode.log"
  result=$?
  printf '%s\t%s\t%s\t%d\n' "$mode" "$began" "$(date -u +%FT%TZ)" "$result" >> "$root/artifacts/sustained-clock/buffer-regressions/exits.tsv"
done
