#!/usr/bin/env zsh
set -euo pipefail
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/1000}"
for fixture in camera six-channel; do
  project="$PWD/artifacts/streaming/qualified-$fixture/Preview.editbay"
  composition=$(jq -r '.sequences[0].composition' "$project")
  "$PWD/artifacts/range-delivery/prepared/bin/editbay-lab" range-delivery "$PWD/artifacts/range-delivery/prepared/bin/editbay" "$project" "$composition" "$PWD/artifacts/range-delivery/prepared/qualified-$fixture" > "artifacts/range-delivery/prepared/$fixture-result.json" 2> "artifacts/range-delivery/prepared/$fixture.log"
done
