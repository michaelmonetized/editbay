#!/usr/bin/env zsh
set -uo pipefail
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
base="$PWD/artifacts/source-preparation/candidate/native"
bin="$PWD/artifacts/source-preparation/candidate/bin"
mkdir -p "$base"
for fixture in camera six-channel; do
  project="$PWD/artifacts/streaming/qualified-$fixture/Preview.editbay"
  if [[ "$fixture" == camera ]]; then source='/home/michael/Downloads/iCloud Photos/IMG_0001.MP4'; else source="$PWD/artifacts/natural-sound/fixtures/qualified-six-channel.mov"; fi
  for mode in prepared device playback; do
    began=$(date -u +%FT%TZ)
    if [[ "$mode" == playback ]]; then
      "$bin/editbay-lab" native-playback "$bin/editbay-studio" "$source" "$base/$fixture-$mode" full > "$base/$fixture-$mode.json" 2> "$base/$fixture-$mode.log"
    else
      "$bin/editbay-lab" "native-$mode" "$bin/editbay-studio" "$project" "$base/$fixture-$mode" > "$base/$fixture-$mode.json" 2> "$base/$fixture-$mode.log"
    fi
    result=$?
    printf '%s\t%s\t%s\t%s\t%d\n' "$fixture" "$mode" "$began" "$(date -u +%FT%TZ)" "$result" >> "$base/exits.tsv"
  done
done
