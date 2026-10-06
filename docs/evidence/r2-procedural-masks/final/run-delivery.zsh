#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
export TMPDIR="$root/artifacts/tmp"
unset LD_LIBRARY_PATH
mkdir -p artifacts/gpu-masks/delivery-bin
cp /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-engine/target/native-build/debug/{editbay-lab,editbay-studio} artifacts/gpu-masks/delivery-bin/
git rev-parse HEAD > artifacts/gpu-masks/delivery-bin/source-commit.txt
sha256sum artifacts/gpu-masks/delivery-bin/{editbay-lab,editbay-studio} > artifacts/gpu-masks/delivery-bin/binaries.sha256
sha256sum Cargo.lock > artifacts/gpu-masks/delivery-bin/lockfile.sha256
for source in camera six; do
  lab="$root/artifacts/gpu-masks/delivery-bin/editbay-lab"
  app="$root/artifacts/gpu-masks/delivery-bin/editbay-studio"
  began=$(date -u +%FT%TZ)
  "$lab" native-masks "$app" "$root/artifacts/gpu-masks/candidate-$source/Mask.editbay" "$root/artifacts/gpu-masks/delivery-native-$source" > "artifacts/gpu-masks/delivery-native-$source.json" 2> "artifacts/gpu-masks/delivery-native-$source.log"
  result=$?
  printf 'native-%s\t%s\t%s\t%d\n' "$source" "$began" "$(date -u +%FT%TZ)" "$result" >> artifacts/gpu-masks/delivery-exits.tsv
done
