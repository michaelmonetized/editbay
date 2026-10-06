#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
mkdir -p artifacts/sound-kernels/active/checks
export TMPDIR="$root/artifacts/tmp"
export CARGO_TARGET_DIR=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-engine/target/native-build
export CARGO_BUILD_JOBS=1
unset RUSTUP_FORCE_ARG0 LD_LIBRARY_PATH
for check in default all-feature clippy fmt; do
  began=$(date -u +%FT%TZ)
  case "$check" in
    default) cargo test --workspace --locked > artifacts/sound-kernels/active/checks/$check.log 2>&1;;
    all-feature)
      export EDITBAY_LIBTORCH=/home/michael/Projects/editbay/artifacts/native-torch/torch
      export LD_LIBRARY_PATH="$EDITBAY_LIBTORCH/lib"
      cargo test --workspace --all-features --locked > artifacts/sound-kernels/active/checks/$check.log 2>&1;;
    clippy) cargo clippy --workspace --all-targets --all-features --locked -- -D warnings > artifacts/sound-kernels/active/checks/$check.log 2>&1;;
    fmt) cargo fmt --all -- --check > artifacts/sound-kernels/active/checks/$check.log 2>&1;;
  esac
  result=$?
  printf '%s\t%s\t%s\t%d\n' "$check" "$began" "$(date -u +%FT%TZ)" "$result" >> artifacts/sound-kernels/active/checks/exits.tsv
done
