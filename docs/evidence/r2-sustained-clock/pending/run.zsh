#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
export TMPDIR="$root/artifacts/tmp"
unset LD_LIBRARY_PATH
mkdir -p artifacts/sustained-clock/two-hour-reason-receipts
receipt="$root/artifacts/sustained-clock/two-hour-reason-receipts"
date -u +%FT%TZ > "$receipt/started.txt"
uname -a > "$receipt/host.txt"
lscpu > "$receipt/cpu.txt"
free -k > "$receipt/memory-before.txt"
cat /proc/pressure/cpu > "$receipt/cpu-pressure-before.txt"
cp artifacts/sustained-clock/reason-bin/{source-commit.txt,binaries.sha256,lockfile.sha256} "$receipt/"
sha256sum artifacts/streaming/fixtures/quiet-45s.mov > "$receipt/source.sha256"
"$root/artifacts/sustained-clock/reason-bin/editbay-lab" sustain-sound "$root/artifacts/streaming/fixtures/quiet-45s.mov" 7200 "$root/artifacts/sustained-clock/two-hours-reason" > "$receipt/result.json" 2> "$receipt/run.log"
result=$?
printf 'sustain-sound-7200\t%s\t%d\n' "$(date -u +%FT%TZ)" "$result" > "$receipt/exit.tsv"
free -k > "$receipt/memory-after.txt"
cat /proc/pressure/cpu > "$receipt/cpu-pressure-after.txt"
