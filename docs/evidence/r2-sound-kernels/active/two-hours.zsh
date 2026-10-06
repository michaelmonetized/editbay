#!/usr/bin/env zsh
set -uo pipefail
root=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker
cd "$root" || exit 1
export TMPDIR="$root/artifacts/tmp"
unset LD_LIBRARY_PATH
base="$root/artifacts/sound-kernels/active"
jq -e '.qualified and .complete and .authored.seconds_requested == 180' "$base/pilot/quiet.json" >/dev/null || exit 1
receipt="$base/two-hour-receipts"
mkdir "$receipt" || exit 1
date -u +%FT%TZ > "$receipt/started.txt"
uname -a > "$receipt/host.txt"
lscpu > "$receipt/cpu.txt"
free -k > "$receipt/memory-before.txt"
cat /proc/pressure/cpu > "$receipt/cpu-pressure-before.txt"
cat /proc/pressure/memory > "$receipt/memory-pressure-before.txt"
cp "$base/"{source-commit.txt,binaries.sha256,lockfile.sha256} "$receipt/"
sha256sum artifacts/streaming/fixtures/quiet-45s.mov > "$receipt/source.sha256"
printf '%s\n' 'All intentional native device fault trials have ended. Ordinary Rust roadmap implementation and qualification may continue; no owned device workers are deliberately stopped, killed or stalled during this baseline. Host pressure and combined owned-process RSS are sampled. Default sound route and all queue/clock limits are unchanged.' > "$receipt/concurrent-work.txt"
"$base/bin/editbay-lab" sustain-sound "$root/artifacts/streaming/fixtures/quiet-45s.mov" 7200 "$base/two-hours" > "$receipt/result.json" 2> "$receipt/run.log"
result=$?
printf 'sustain-sound-7200\t%s\t%d\n' "$(date -u +%FT%TZ)" "$result" > "$receipt/exit.tsv"
free -k > "$receipt/memory-after.txt"
cat /proc/pressure/cpu > "$receipt/cpu-pressure-after.txt"
cat /proc/pressure/memory > "$receipt/memory-pressure-after.txt"
