#!/usr/bin/env zsh
set -uo pipefail
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
for mode in range range-jobs; do
  "$PWD/artifacts/range-delivery/final3/bin/editbay-lab" "native-$mode" "$PWD/artifacts/range-delivery/final3/bin/editbay-studio" "$PWD/artifacts/streaming/qualified-camera/Preview.editbay" "$PWD/artifacts/range-delivery/prepared/qualified-camera/Full-48000.mov" "$PWD/artifacts/range-delivery/final3/native/camera-$mode-repeat" > "artifacts/range-delivery/final3/native/camera-$mode-repeat.json" 2> "artifacts/range-delivery/final3/native/camera-$mode-repeat.log"
  result=$?
  printf '%s\t%d\n' "$mode" "$result" >> artifacts/range-delivery/final3/repeat-camera-exits.tsv
done
