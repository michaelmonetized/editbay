#!/usr/bin/env zsh
set -u
cd /home/michael/Projects/editbay/artifacts/worktrees/r2-sound-worker || exit 1
while [[ ! -f artifacts/recovery-publication/checks/workspace/exits.tsv ]] || ! rg -q '^fmt' artifacts/recovery-publication/checks/workspace/exits.tsv; do sleep 1; done
unset LD_LIBRARY_PATH
export TMPDIR="$PWD/artifacts/tmp"
base="$PWD/artifacts/recovery-publication/candidate"
"$base/bin/editbay-lab" native-workspace "$base/bin/editbay-studio" "$base/reproduce" 2 > "$base/reproduce.json" 2> "$base/reproduce.log" &
lab_pid=$!
while kill -0 "$lab_pid" 2>/dev/null; do
  for trace in "$base"/reproduce/trial-*/before-kill.jsonl(N); do
    app_pid=$(head -n 1 "$trace" | jq -r '.pid')
    for task in /proc/$app_pid/task/*(N); do
      name=$(cat "$task/comm" 2>/dev/null)
      if [[ "$name" == editbay-recover* || "$name" == editbay-files ]]; then
        printf '%s\t%s\t%s\t%s\n' "$(date -u +%FT%T.%NZ)" "$task" "$name" "$(cat "$task/wchan" 2>/dev/null)" >> "$base/reproduce-workers.tsv"
      fi
    done
  done
  sleep 0.25
done
wait "$lab_pid"
result_code=$?
printf '%s\t%s\n' "$(date -u +%FT%TZ)" "$result_code" > "$base/reproduce-exit.tsv"
