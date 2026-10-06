#!/usr/bin/env zsh
set -euo pipefail
cd /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
src=artifacts/active-pictures
dst=docs/evidence/r2-active-pictures
mkdir -p "$dst/earlier" "$dst/native-camera" "$dst/native-six"
for name in parent-commit.txt checked-commit.txt checks-start.txt checks-end.txt checks.status machine.txt inputs.sha256 compiled-inputs.sha256 source-timing-binaries.sha256 source-timing-commit.txt final-driver-commit.txt final-driver.sha256 native-trials-start.txt checks.sh native-trials.zsh source-timing-stages.json timing-stages.json; do
  cp "$src/$name" "$dst/"
done
for name in default-tests all-feature-tests all-feature-clippy fmt source-timing-build final-driver-build final-driver-clippy; do
  gzip -nc "$src/$name.log" > "$dst/$name.log.gz"
done
for trial in pilot-native-camera timing-native-camera source-timing-native-camera final-native-camera final-native-six verified-native-six; do
  target="$dst/earlier/$trial"
  [[ "$trial" == final-native-camera ]] && target="$dst/native-camera"
  [[ "$trial" == verified-native-six ]] && target="$dst/native-six"
  mkdir -p "$target"
  cp "$src/$trial.log" "$target/run.log"
  if [[ -s "$src/$trial.json" ]]; then
    gzip -nc "$src/$trial.json" > "$target/complete-observations.json.gz"
    jq 'walk(if type=="object" then del(.controls) | if (.clips|type)=="array" then .clip_count=(.clips|length) | del(.clips) else . end else . end)' "$src/$trial.json" > "$target/receipt.json"
  fi
  for artifact_file in "$src/$trial/"*.jsonl(N) "$src/$trial/"*.editbay(N); do
    gzip -nc "$artifact_file" > "$target/${artifact_file:t}.gz"
  done
  if [[ "$trial" == final-native-camera || "$trial" == verified-native-six ]]; then
    cp "$src/$trial/long-native.png" "$src/$trial/long-800x600.png" "$target/"
  fi
done
for name in pilot-binaries.sha256 pilot-source-commit.txt timing-binary.sha256 timing-source-commit.txt; do
  cp "$src/$name" "$dst/earlier/"
done
sha256sum "$src"/{pilot-native-camera,timing-native-camera,source-timing-native-camera,final-native-camera,verified-native-six}/Long.mov artifacts/sound-index/qualified-native-{camera,six}/Long.mov > "$dst/master-files.sha256"
date -u +%FT%TZ > "$dst/trials-finished.txt"
