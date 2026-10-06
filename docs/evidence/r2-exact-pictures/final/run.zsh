#!/usr/bin/env zsh
set -u
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
base="$PWD/artifacts/dense-picture-cache/compute-trials"
bin="$PWD/artifacts/dense-picture-cache/compute-bin"
run_case() {
    local case_name="$1"
    shift
    local began="$(date -u +%FT%TZ)"
    "$@" > "$base/$case_name.json" 2> "$base/$case_name.log"
    local result_code=$?
    printf '%s\t%s\t%s\t%s\n' "$case_name" "$began" "$(date -u +%FT%TZ)" "$result_code" >> "$base/exits.tsv"
}
run_case store-camera "$bin/editbay-lab" picture-store artifacts/sound-index/qualified-camera/Long.editbay 1df0adc3-dca7-47d0-ac3a-b8dc1bb93496 "$base/store-camera" "$bin/editbay"
run_case store-six "$bin/editbay-lab" picture-store artifacts/sound-index/qualified-six/Long.editbay cc86f528-b4f3-4516-8c6d-acdf7036d67c "$base/store-six" "$bin/editbay-studio"
run_case native-camera "$bin/editbay-lab" native-cached "$bin/editbay-studio" artifacts/sound-index/qualified-camera/Long.editbay "$base/native-camera"
run_case native-six "$bin/editbay-lab" native-cached "$bin/editbay-studio" artifacts/sound-index/qualified-six/Long.editbay "$base/native-six"
date -u +%FT%TZ > "$base/finished.txt"
cat /proc/meminfo > "$base/host-memory-after.txt"
cat /proc/pressure/memory > "$base/host-pressure-after.txt"
