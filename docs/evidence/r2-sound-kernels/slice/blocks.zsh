#!/usr/bin/env zsh
set -uo pipefail
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
bin="$PWD/artifacts/sound-kernels/slice/bin"
for fixture in camera six; do
  if [[ "$fixture" == camera ]]; then source='/home/michael/Downloads/iCloud Photos/IMG_0001.MP4'; else source="$PWD/artifacts/natural-sound/fixtures/qualified-six-channel.mov"; fi
  "$bin/editbay-lab" sound-blocks-worker "$source" "$bin/editbay-studio" > "artifacts/sound-kernels/slice/$fixture-blocks.json" 2> "artifacts/sound-kernels/slice/$fixture-blocks.log"
  result=$?
  printf '%s\t%d\n' "$fixture" "$result" >> artifacts/sound-kernels/slice/blocks-exits.tsv
done
