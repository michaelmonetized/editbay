#!/usr/bin/env zsh
set -uo pipefail
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
base="$PWD/artifacts/sound-kernels/compiled"
bin="$base/bin"
for fixture in camera six-channel; do
  if [[ "$fixture" == camera ]]; then composition=d1410cfa-579c-40e1-8c6a-4301d22b48e5; else composition=8692a2b3-18c2-4d6b-ac7c-b5e2b5f36983; fi
  began=$(date -u +%FT%TZ)
  "$bin/editbay-lab" range-delivery "$bin/editbay" "$PWD/artifacts/streaming/qualified-$fixture/Preview.editbay" "$composition" "$base/ranges-$fixture" "$PWD/artifacts/range-delivery/prepared/qualified-$fixture" > "$base/ranges-$fixture.json" 2> "$base/ranges-$fixture.log"
  result=$?
  printf '%s\t%s\t%s\t%d\n' "$fixture" "$began" "$(date -u +%FT%TZ)" "$result" >> "$base/ranges-exits.tsv"
done
