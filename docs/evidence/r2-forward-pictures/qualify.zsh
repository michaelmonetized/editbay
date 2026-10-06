#!/usr/bin/env zsh
set -euo pipefail
cd /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
export TMPDIR="$PWD/artifacts/tmp"
export CARGO_TARGET_DIR=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-engine/target/native-build
export CARGO_BUILD_JOBS=2
export EDITBAY_LIBTORCH=/home/michael/Projects/editbay/artifacts/native-torch/torch
export LD_LIBRARY_PATH="$EDITBAY_LIBTORCH/lib"
dst=artifacts/forward-pictures
date -u +%FT%TZ > "$dst/started.txt"
git ls-files crates Cargo.toml Cargo.lock | xargs sha256sum > "$dst/compiled-inputs.sha256"
env -u RUSTUP_FORCE_ARG0 cargo fmt --all -- --check > "$dst/fmt.log" 2>&1
env -u RUSTUP_FORCE_ARG0 cargo test --workspace --locked > "$dst/default-tests.log" 2>&1
env -u RUSTUP_FORCE_ARG0 cargo test --workspace --all-features --locked > "$dst/all-feature-tests.log" 2>&1
env -u RUSTUP_FORCE_ARG0 cargo clippy --workspace --all-targets --all-features --locked -- -D warnings > "$dst/all-feature-clippy.log" 2>&1
env -u RUSTUP_FORCE_ARG0 cargo build --release --locked -p editbay-cli -p editbay-app -p editbay-lab > "$dst/build.log" 2>&1
mkdir "$dst/qualified-bin"
cp "$CARGO_TARGET_DIR/release/"{editbay,editbay-studio,editbay-lab} "$dst/qualified-bin/"
sha256sum "$dst/qualified-bin/"* > "$dst/binaries.sha256"
sha256sum -c "$dst/compiled-inputs.sha256" > "$dst/compiled-inputs-verified.log"
unset LD_LIBRARY_PATH
lab="$PWD/$dst/qualified-bin/editbay-lab"
app="$PWD/$dst/qualified-bin/editbay-studio"
parent="$PWD/artifacts/active-pictures/source-timing-bin/editbay-studio"
date -u +%FT%TZ > "$dst/trials-started.txt"
for fixture in camera six; do
  if [[ "$fixture" == camera ]]; then
    media='/home/michael/Downloads/iCloud Photos/IMG_0001.MP4'
    project=artifacts/streaming/qualified-camera/Preview.editbay
    worker="$app"
  else
    media=artifacts/natural-sound/fixtures/qualified-six-channel.mov
    project=artifacts/streaming/qualified-six-channel/Preview.editbay
    worker="$PWD/$dst/qualified-bin/editbay"
  fi
  "$lab" picture-worker "$media" "$worker" > "$dst/$fixture-worker.json" 2> "$dst/$fixture-worker.log"
  "$lab" native-continuous "$parent" "$project" "$dst/parent-native-$fixture" > "$dst/parent-native-$fixture.json" 2> "$dst/parent-native-$fixture.log"
  "$lab" native-continuous "$app" "$project" "$dst/qualified-native-$fixture" > "$dst/qualified-native-$fixture.json" 2> "$dst/qualified-native-$fixture.log"
  "$lab" native-long "$app" "artifacts/sound-index/qualified-$fixture/Long.editbay" "$dst/qualified-long-$fixture" > "$dst/qualified-long-$fixture.json" 2> "$dst/qualified-long-$fixture.log"
done
date -u +%FT%TZ > "$dst/finished.txt"
print completed > "$dst/qualification.status"
