#!/usr/bin/env zsh
set -euo pipefail
unset RUSTUP_FORCE_ARG0
export TMPDIR="$PWD/artifacts/tmp"
export CARGO_TARGET_DIR=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-engine/target/native-build
export CARGO_BUILD_JOBS=1
cargo fmt --all -- --check > artifacts/range-delivery/prepared/fmt.log 2>&1
cargo test --workspace --locked > artifacts/range-delivery/prepared/default-tests.log 2>&1
export EDITBAY_LIBTORCH=/home/michael/Projects/editbay/artifacts/native-torch/torch
export LD_LIBRARY_PATH="$EDITBAY_LIBTORCH/lib"
cargo test --workspace --all-features --locked > artifacts/range-delivery/prepared/all-feature-tests.log 2>&1
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings > artifacts/range-delivery/prepared/clippy.log 2>&1
