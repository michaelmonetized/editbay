#!/usr/bin/env zsh
set -uo pipefail
unset LD_LIBRARY_PATH
root="$PWD"
export TMPDIR="$root/artifacts/tmp"
base="$root/artifacts/sound-kernels/compiled/native-repeat"
bin="$root/artifacts/sound-kernels/compiled/bin"
mkdir -p "$base"
for fixture in camera six-channel; do
  project="$root/artifacts/streaming/qualified-$fixture/Preview.editbay"
  if [[ "$fixture" == camera ]]; then source='/home/michael/Downloads/iCloud Photos/IMG_0001.MP4'; else source="$root/artifacts/natural-sound/fixtures/qualified-six-channel.mov"; fi
  for mode in prepared device playback; do
    if [[ "$fixture" == six-channel && "$mode" != prepared ]]; then continue; fi
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
