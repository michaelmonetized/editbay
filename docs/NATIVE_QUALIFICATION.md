# Native qualification

`editbay-lab native-workspace APP NEW_DIRECTORY 100` drives the installed native
window and file chooser. It creates 4,000 real catalog documents, then kills,
recovers and reopens 50 saved and 50 untitled projects. Original files and
acknowledged checkpoints must remain exact; recovered documents have separate
identities. Input acceptance p95 must stay at or below 50 ms.

Window creation has 30 seconds. Ordinary UI acknowledgement retains 10 seconds;
durable storage completion has a separate 120-second deadline. Actual startup
and storage waits are reported independently. On 2026-10-07, a recovered-copy
trial exposed a disk worker blocked in `btrfs_sync_log` beyond the former generic
10-second wait while native UI frames continued at 107–143 microseconds. Earlier
timeouts remain failed evidence. A longer storage wait does not acknowledge a
checkpoint or accept a recovered file before the real operation finishes.

Set `EDITBAY_LAB_DIAGNOSTICS_ROOT` to an existing project-local directory to keep
opt-in observations on another filesystem. Qualification used a bounded 512 MiB,
non-swapping tmpfs for diagnostic capture. Every project, original save,
checkpoint and recovered copy stays on the real disk. After each native process
is killed and waited, capture files are copied and synced into the requested
evidence directory; archive errors fail the run. Input timestamps, dropped-record
rejection and persistence checks remain unchanged. Compare capture/archive hashes
before unmounting the diagnostic filesystem.
