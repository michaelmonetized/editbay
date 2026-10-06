#!/usr/bin/env zsh
set -uo pipefail
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
base="$PWD/artifacts/source-preparation/candidate/delivery-faults"
mkdir -p "$base"
for fixture in camera six-channel; do
  project="$PWD/artifacts/streaming/qualified-$fixture/Preview.editbay"
  composition=$(jq -r '.sequences[0].composition' "$project")
  for mode in cancel kill stall collision cancel-verify source-change cancel-preparation; do
    "$PWD/artifacts/source-preparation/candidate/bin/editbay-lab" range-fault "$project" "$composition" "$base/$fixture-$mode.mov" "$mode" > "$base/$fixture-$mode.json" 2> "$base/$fixture-$mode.log"
    printf '%s\t%s\t%d\n' "$fixture" "$mode" "$?" >> "$base/exits.tsv"
  done
  "$PWD/artifacts/source-preparation/candidate/bin/editbay-lab" delivery-protocol "$project" "$composition" > "$base/$fixture-protocol.json" 2> "$base/$fixture-protocol.log"
  printf '%s\tprotocol\t%d\n' "$fixture" "$?" >> "$base/exits.tsv"
done
