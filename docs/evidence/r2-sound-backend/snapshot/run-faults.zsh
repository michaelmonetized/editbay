#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
export TMPDIR="$root/artifacts/tmp"
unset LD_LIBRARY_PATH
base="$root/artifacts/sound-backend/faults"
lab="$root/artifacts/sound-backend/snapshot-bin/editbay-lab"
mkdir -p "$base"
for fixture in camera six; do
  case "$fixture" in
    camera) source='/home/michael/Downloads/iCloud Photos/IMG_0001.MP4';;
    six) source="$root/artifacts/natural-sound/fixtures/qualified-six-channel.mov";;
  esac
  for mode in full cancel kill underrun kill-device stall-device cancel-stalled-device prepare-cancel resume-device; do
    began=$(date -u +%FT%TZ)
    "$lab" stream-sound "$source" "$mode" > "$base/$fixture-$mode.json" 2> "$base/$fixture-$mode.log"
    result=$?
    printf '%s-%s\t%s\t%s\t%d\n' "$fixture" "$mode" "$began" "$(date -u +%FT%TZ)" "$result" >> "$base/exits.tsv"
  done
done
