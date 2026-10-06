#!/usr/bin/env zsh
set -uo pipefail
unset LD_LIBRARY_PATH
root="$PWD"
export TMPDIR="$root/artifacts/tmp"
base="$root/artifacts/range-delivery/final3/native"
bin="$root/artifacts/range-delivery/final3/bin"
mkdir -p "$base"
for fixture in camera six-channel; do
  project="$root/artifacts/streaming/qualified-$fixture/Preview.editbay"
  reference="$root/artifacts/range-delivery/prepared/qualified-$fixture/Full-48000.mov"
  for mode in range range-jobs device; do
    began=$(date -u +%FT%TZ)
    if [[ "$mode" == device ]]; then
      "$bin/editbay-lab" native-device "$bin/editbay-studio" "$project" "$base/$fixture-$mode" > "$base/$fixture-$mode.json" 2> "$base/$fixture-$mode.log"
    else
      "$bin/editbay-lab" "native-$mode" "$bin/editbay-studio" "$project" "$reference" "$base/$fixture-$mode" > "$base/$fixture-$mode.json" 2> "$base/$fixture-$mode.log"
    fi
    result=$?
    printf '%s\t%s\t%s\t%s\t%d\n' "$fixture" "$mode" "$began" "$(date -u +%FT%TZ)" "$result" >> "$base/exits.tsv"
  done
done
