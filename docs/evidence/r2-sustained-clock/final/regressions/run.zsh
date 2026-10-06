#!/usr/bin/env zsh
set -u
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
base="$PWD/artifacts/sustained-clock/reason-regressions"
bin="$PWD/artifacts/sustained-clock/reason-bin"
run_case() {
    local case_name="$1"
    shift
    local began="$(date -u +%FT%TZ)"
    "$@" > "$base/$case_name.json" 2> "$base/$case_name.log"
    local result_code=$?
    printf '%s\t%s\t%s\t%s\n' "$case_name" "$began" "$(date -u +%FT%TZ)" "$result_code" >> "$base/exits.tsv"
}
run_case prepared-camera "$bin/editbay-lab" native-prepared "$bin/editbay-studio" artifacts/streaming/qualified-camera/Preview.editbay "$base/prepared-camera"
run_case prepared-six "$bin/editbay-lab" native-prepared "$bin/editbay-studio" artifacts/streaming/qualified-six-channel/Preview.editbay "$base/prepared-six"
run_case playback-camera "$bin/editbay-lab" native-playback "$bin/editbay-studio" '/home/michael/Downloads/iCloud Photos/IMG_0001.MP4' "$base/playback-camera"
run_case playback-six "$bin/editbay-lab" native-playback "$bin/editbay-studio" artifacts/natural-sound/fixtures/qualified-six-channel.mov "$base/playback-six"
date -u +%FT%TZ > "$base/finished.txt"
cat /proc/meminfo > "$base/host-memory-after.txt"
cat /proc/pressure/cpu > "$base/host-cpu-after.txt"
