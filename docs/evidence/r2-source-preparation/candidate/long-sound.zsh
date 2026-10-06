#!/usr/bin/env zsh
set -uo pipefail
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
base="$PWD/artifacts/source-preparation/candidate"
for fixture in camera six-channel; do
  began=$(date -u +%FT%TZ)
  "$base/bin/editbay-lab" long-sound "$PWD/artifacts/streaming/qualified-$fixture/Preview.editbay" "$PWD/artifacts/range-delivery/prepared/qualified-$fixture/Full-48000.mov" "$base/long-$fixture" 256 > "$base/long-$fixture.json" 2> "$base/long-$fixture.log"
  result=$?
  printf '%s\t%s\t%s\t%d\n' "$fixture" "$began" "$(date -u +%FT%TZ)" "$result" >> "$base/long-exits.tsv"
done
