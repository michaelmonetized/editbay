#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
export TMPDIR="$root/artifacts/tmp"
unset LD_LIBRARY_PATH
base="$root/artifacts/sound-backend/native"
bin="$root/artifacts/sound-backend/snapshot-bin"
mkdir -p "$base"
for fixture in camera six; do
  case "$fixture" in
    camera) project="$root/artifacts/streaming/qualified-camera/Preview.editbay"; source='/home/michael/Downloads/iCloud Photos/IMG_0001.MP4';;
    six) project="$root/artifacts/streaming/qualified-six-channel/Preview.editbay"; source="$root/artifacts/natural-sound/fixtures/qualified-six-channel.mov";;
  esac
  for mode in prepared playback device; do
    began=$(date -u +%FT%TZ)
    input="$project"
    if [[ "$mode" == playback ]]; then input="$source"; fi
    "$bin/editbay-lab" "native-$mode" "$bin/editbay-studio" "$input" "$base/$fixture-$mode" > "$base/$fixture-$mode.json" 2> "$base/$fixture-$mode.log"
    result=$?
    printf '%s-%s\t%s\t%s\t%d\n' "$fixture" "$mode" "$began" "$(date -u +%FT%TZ)" "$result" >> "$base/exits.tsv"
  done
done
