#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
mkdir -p artifacts/gpu-masks/final-checks
export TMPDIR="$root/artifacts/tmp"
export CARGO_TARGET_DIR=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-engine/target/native-build
export CARGO_BUILD_JOBS=1
unset RUSTUP_FORCE_ARG0
unset LD_LIBRARY_PATH
began=$(date -u +%FT%TZ)
cargo test --workspace --locked > artifacts/gpu-masks/final-checks/default.log 2>&1
result=$?
printf 'default\t%s\t%s\t%d\n' "$began" "$(date -u +%FT%TZ)" "$result" >> artifacts/gpu-masks/final-checks/exits.tsv
export EDITBAY_LIBTORCH=/home/michael/Projects/editbay/artifacts/native-torch/torch
export LD_LIBRARY_PATH="$EDITBAY_LIBTORCH/lib"
for check in all-feature clippy fmt; do
  began=$(date -u +%FT%TZ)
  case "$check" in
    all-feature) cargo test --workspace --all-features --locked > artifacts/gpu-masks/final-checks/$check.log 2>&1;;
    clippy) cargo clippy --workspace --all-targets --all-features --locked -- -D warnings > artifacts/gpu-masks/final-checks/$check.log 2>&1;;
    fmt) cargo fmt --all -- --check > artifacts/gpu-masks/final-checks/$check.log 2>&1;;
  esac
  result=$?
  printf '%s\t%s\t%s\t%d\n' "$check" "$began" "$(date -u +%FT%TZ)" "$result" >> artifacts/gpu-masks/final-checks/exits.tsv
done
