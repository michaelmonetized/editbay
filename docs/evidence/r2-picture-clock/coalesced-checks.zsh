#!/usr/bin/env zsh
set -euo pipefail
cd /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
export TMPDIR="$PWD/artifacts/tmp"
export CARGO_TARGET_DIR=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-engine/target/native-build
export CARGO_BUILD_JOBS=2
export EDITBAY_LIBTORCH=/home/michael/Projects/editbay/artifacts/native-torch/torch
export LD_LIBRARY_PATH="$EDITBAY_LIBTORCH/lib"
dst=artifacts/picture-clock/coalesced-validation
mkdir "$dst/checks"
git rev-parse HEAD > "$dst/checks/commit.txt"
git ls-files crates Cargo.toml Cargo.lock | xargs sha256sum > "$dst/checks/compiled-inputs.sha256"
date -u +%FT%TZ > "$dst/checks/started.txt"
env -u RUSTUP_FORCE_ARG0 cargo fmt --all -- --check > "$dst/checks/fmt.log" 2>&1
env -u RUSTUP_FORCE_ARG0 cargo test --workspace --locked > "$dst/checks/default-tests.log" 2>&1
env -u RUSTUP_FORCE_ARG0 cargo test --workspace --all-features --locked > "$dst/checks/all-feature-tests.log" 2>&1
env -u RUSTUP_FORCE_ARG0 cargo clippy --workspace --all-targets --all-features --locked -- -D warnings > "$dst/checks/all-feature-clippy.log" 2>&1
sha256sum -c "$dst/checks/compiled-inputs.sha256" > "$dst/checks/compiled-inputs-verified.log"
date -u +%FT%TZ > "$dst/checks/finished.txt"
