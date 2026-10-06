#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
export TMPDIR="$root/artifacts/tmp"
unset LD_LIBRARY_PATH
lab="$root/artifacts/sustained-clock/rate-bin/editbay-lab"
for fixture in camera six quiet; do
  seconds=6
  case "$fixture" in
    camera) source='/home/michael/Downloads/iCloud Photos/IMG_0001.MP4';;
    six) source="$root/artifacts/natural-sound/fixtures/qualified-six-channel.mov";;
    quiet) source="$root/artifacts/streaming/fixtures/quiet-45s.mov"; seconds=180;;
  esac
  began=$(date -u +%FT%TZ)
  "$lab" sustain-sound "$source" "$seconds" "$root/artifacts/sustained-clock/rate/$fixture" > "$root/artifacts/sustained-clock/rate/$fixture.json" 2> "$root/artifacts/sustained-clock/rate/$fixture.log"
  result=$?
  printf '%s\t%s\t%s\t%d\n' "$fixture" "$began" "$(date -u +%FT%TZ)" "$result" >> "$root/artifacts/sustained-clock/rate/exits.tsv"
done
