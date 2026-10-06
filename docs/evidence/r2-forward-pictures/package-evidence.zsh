#!/usr/bin/env zsh
set -euo pipefail
cd /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
src=artifacts/forward-pictures
dst=docs/evidence/r2-forward-pictures
mkdir -p "$dst"
for name in parent-commit.txt qualified-source-commit.txt compiled-inputs.sha256 binaries.sha256 started.txt trials-started.txt finished.txt qualification.status host-memory-during.txt host-memory-pressure-during.txt host-memory-observed-at.txt; do
  cp "$src/$name" "$dst/"
done
sed '/cat \/proc\/meminfo/d; /cat \/proc\/pressure\/memory/d' "$src/qualify.zsh" > "$dst/qualify.zsh"
for name in default-tests all-feature-tests all-feature-clippy fmt build; do
  gzip -nc "$src/$name.log" > "$dst/$name.log.gz"
done
for fixture in camera six; do
  cp "$src/$fixture-worker.json" "$src/$fixture-worker.log" "$dst/"
done
for trial in parent-native-camera qualified-native-camera parent-native-six qualified-native-six qualified-long-camera qualified-long-six; do
  target="$dst/$trial"
  mkdir -p "$target"
  cp "$src/$trial.log" "$target/run.log"
  gzip -nc "$src/$trial.json" > "$target/complete-observations.json.gz"
  jq 'walk(if type=="object" then del(.controls) | (if (.clips|type)=="array" then .clip_count=(.clips|length) | del(.clips) else . end) | (if (.displayed_observations|type)=="array" then .displayed_observation_count=(.displayed_observations|length) | del(.displayed_observations) else . end) else . end)' "$src/$trial.json" > "$target/receipt.json"
  for artifact_file in "$src/$trial/"*.jsonl(N) "$src/$trial/"*.editbay(N); do
    gzip -nc "$artifact_file" > "$target/${artifact_file:t}.gz"
  done
  cp "$src/$trial/"*.png "$target/"
done
sha256sum "$src/qualified-long-"{camera,six}/Long.mov artifacts/sound-index/qualified-native-{camera,six}/Long.mov > "$dst/master-files.sha256"
sha256sum Cargo.lock '/home/michael/Downloads/iCloud Photos/IMG_0001.MP4' artifacts/natural-sound/fixtures/qualified-six-channel.mov artifacts/streaming/qualified-{camera,six-channel}/Preview.editbay artifacts/sound-index/qualified-{camera,six}/Long.editbay > "$dst/inputs.sha256"
cp artifacts/active-pictures/machine.txt "$dst/"
cp artifacts/active-pictures/source-timing-binaries.sha256 "$dst/parent-binaries.sha256"
