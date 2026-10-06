#!/bin/zsh
set -euo pipefail
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
lab="$PWD/artifacts/sound-index/qualified-bin/editbay-lab"
app="$PWD/artifacts/sound-index/qualified-bin/editbay-studio"
receipt_dir=docs/evidence/r2-sound-index
sha256sum -c artifacts/sound-index/binaries.sha256 > artifacts/sound-index/binaries-verified.log
sha256sum -c artifacts/sound-index/qualification-inputs.sha256 > artifacts/sound-index/inputs-verified.log
sha256sum -c artifacts/sound-index/compiled-inputs.sha256 > artifacts/sound-index/compiled-inputs-verified.log
date -u > "$receipt_dir/trials-started.txt"
for fixture in camera six; do
  if [[ "$fixture" = camera ]]; then
    media='/home/michael/Downloads/iCloud Photos/IMG_0001.MP4'
    project=artifacts/streaming/qualified-camera/Preview.editbay
    reference=artifacts/delivery/qualified-camera-recovered.mov
  else
    media=artifacts/natural-sound/fixtures/qualified-six-channel.mov
    project=artifacts/streaming/qualified-six-channel/Preview.editbay
    reference=artifacts/delivery/qualified-six-full.mov
  fi
  cut="artifacts/sound-index/qualified-$fixture"
  "$lab" long-sound "$project" "$reference" "$cut" 128 > "$receipt_dir/$fixture-long.json" 2> "$receipt_dir/$fixture-long.log"
  jq -e '.qualified == true' "$receipt_dir/$fixture-long.json" > /dev/null
  "$lab" sound-blocks-worker "$media" > "$receipt_dir/$fixture-pcm.json" 2> "$receipt_dir/$fixture-pcm.log"
  jq -e '.qualified == true' "$receipt_dir/$fixture-pcm.json" > /dev/null
  for mode in cancel prepare-cancel kill-device; do
    "$lab" stream-sound "$media" "$mode" > "$receipt_dir/$fixture-$mode.json" 2> "$receipt_dir/$fixture-$mode.log"
    jq -e '.qualified == true' "$receipt_dir/$fixture-$mode.json" > /dev/null
  done
  native="artifacts/sound-index/qualified-native-$fixture"
  "$lab" native-long "$app" "$cut/Long.editbay" "$native" > "$receipt_dir/$fixture-native.json" 2> "$receipt_dir/$fixture-native.log"
  jq -e '.qualified == true' "$receipt_dir/$fixture-native.json" > /dev/null
  composition=$(jq -r .composition "$receipt_dir/$fixture-native.json")
  "$lab" timeline-compare "$native/Long.editbay" "$composition" "$reference" "$native/Long.mov" > "$receipt_dir/$fixture-independent.json" 2> "$receipt_dir/$fixture-independent.log"
  jq -e '.qualified == true' "$receipt_dir/$fixture-independent.json" > /dev/null
done
"$lab" long-sound artifacts/streaming/qualified-six-channel/Preview.editbay artifacts/delivery/qualified-six-full.mov artifacts/sound-index/qualified-six-256 256 > "$receipt_dir/six-256.json" 2> "$receipt_dir/six-256.log"
jq -e '.qualified == true' "$receipt_dir/six-256.json" > /dev/null
date -u > "$receipt_dir/trials-finished.txt"
printf '%s\n' passed > artifacts/sound-index/final-qualification.status
