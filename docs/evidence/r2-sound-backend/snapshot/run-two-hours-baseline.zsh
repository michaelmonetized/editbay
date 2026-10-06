#!/usr/bin/env zsh
set -u
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
export TMPDIR="$root/artifacts/tmp"
unset LD_LIBRARY_PATH
receipt="$root/artifacts/sound-backend/two-hour-baseline-receipts"
mkdir -p "$receipt"
date -u +%FT%TZ > "$receipt/started.txt"
uname -a > "$receipt/host.txt"
lscpu > "$receipt/cpu.txt"
free -k > "$receipt/memory-before.txt"
cat /proc/pressure/cpu > "$receipt/cpu-pressure-before.txt"
cp artifacts/sound-backend/snapshot-bin/{source-commit.txt,binaries.sha256,lockfile.sha256} "$receipt/"
sha256sum artifacts/streaming/fixtures/quiet-45s.mov > "$receipt/source.sha256"
printf '%s\n' 'All deliberate native sound fault trials have ended. Rust roadmap builds and picture/export UI work may continue; no owned sound clients are deliberately killed or stalled during this baseline. Host pressure and combined owned-process RSS are sampled. Default actual sound route is unchanged.' > "$receipt/concurrent-work.txt"
"$root/artifacts/sound-backend/snapshot-bin/editbay-lab" sustain-sound "$root/artifacts/streaming/fixtures/quiet-45s.mov" 7200 "$root/artifacts/sound-backend/two-hours-baseline" > "$receipt/result.json" 2> "$receipt/run.log"
result=$?
printf 'sustain-sound-7200\t%s\t%d\n' "$(date -u +%FT%TZ)" "$result" > "$receipt/exit.tsv"
free -k > "$receipt/memory-after.txt"
cat /proc/pressure/cpu > "$receipt/cpu-pressure-after.txt"
