#!/usr/bin/env zsh
set -euo pipefail
cd /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
export TMPDIR="$PWD/artifacts/tmp"
export CARGO_TARGET_DIR=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-engine/target/native-build
export CARGO_BUILD_JOBS=2
export EDITBAY_LIBTORCH=/home/michael/Projects/editbay/artifacts/native-torch/torch
export LD_LIBRARY_PATH="$EDITBAY_LIBTORCH/lib"
git rev-parse HEAD > artifacts/active-pictures/checked-commit.txt
date -u +%FT%TZ > artifacts/active-pictures/checks-start.txt
env -u RUSTUP_FORCE_ARG0 cargo fmt --all -- --check > artifacts/active-pictures/fmt.log 2>&1
env -u RUSTUP_FORCE_ARG0 cargo test --workspace --locked > artifacts/active-pictures/default-tests.log 2>&1
env -u RUSTUP_FORCE_ARG0 cargo test --workspace --all-features --locked > artifacts/active-pictures/all-feature-tests.log 2>&1
env -u RUSTUP_FORCE_ARG0 cargo clippy --workspace --all-targets --all-features --locked -- -D warnings > artifacts/active-pictures/all-feature-clippy.log 2>&1
date -u +%FT%TZ > artifacts/active-pictures/checks-end.txt
print passed > artifacts/active-pictures/checks.status
