#!/usr/bin/env zsh
set -euo pipefail
cd /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
lab="$PWD/artifacts/active-pictures/source-timing-bin/editbay-lab"
app="$PWD/artifacts/active-pictures/source-timing-bin/editbay-studio"
sha256sum -c artifacts/active-pictures/source-timing-binaries.sha256 > artifacts/active-pictures/binaries-verified.log
date -u +%FT%TZ > artifacts/active-pictures/native-trials-start.txt
for fixture in camera six; do
  "$lab" native-long "$app" "artifacts/sound-index/qualified-$fixture/Long.editbay" "artifacts/active-pictures/final-native-$fixture" > "artifacts/active-pictures/final-native-$fixture.json" 2> "artifacts/active-pictures/final-native-$fixture.log"
done
date -u +%FT%TZ > artifacts/active-pictures/native-trials-end.txt
sha256sum artifacts/active-pictures/{pilot-native-camera,timing-native-camera,source-timing-native-camera,final-native-camera,final-native-six}/Long.mov artifacts/sound-index/qualified-native-{camera,six}/Long.mov > artifacts/active-pictures/master-files.sha256
print completed > artifacts/active-pictures/native-trials.status
