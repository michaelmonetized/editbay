#!/usr/bin/env zsh
set -euo pipefail
cd /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
export TMPDIR="$PWD/artifacts/tmp"
export CARGO_TARGET_DIR=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-engine/target/native-build
export CARGO_BUILD_JOBS=2
dst=artifacts/picture-clock
mkdir "$dst/final-build"
git rev-parse HEAD > "$dst/final-build/commit.txt"
git ls-files crates Cargo.toml Cargo.lock | xargs sha256sum > "$dst/final-build/compiled-inputs.sha256"
date -u +%FT%TZ > "$dst/final-build/started.txt"
env -u RUSTUP_FORCE_ARG0 cargo build --release --locked -p editbay-cli -p editbay-app -p editbay-lab > "$dst/final-build/build.log" 2>&1
mkdir "$dst/qualified-bin"
cp "$CARGO_TARGET_DIR/release/"{editbay,editbay-studio,editbay-lab} "$dst/qualified-bin/"
sha256sum "$dst/qualified-bin/"* > "$dst/final-build/binaries.sha256"
sha256sum -c "$dst/final-build/compiled-inputs.sha256" > "$dst/final-build/compiled-inputs-verified.log"
date -u +%FT%TZ > "$dst/final-build/finished.txt"
