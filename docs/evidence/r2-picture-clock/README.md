# Prepared native pictures and actual draw completion

Issue [#38](https://github.com/michaelmonetized/editbay/issues/38) follows
[PR #39](https://github.com/michaelmonetized/editbay/pull/39). Runtime, driver and
frozen final binaries are from `5578d568c134ca3ca1b99b825320f6dccab5c2d8`.
See [contracts](../../PREPARED_PICTURES.md), [gates](gates.json), source hashes,
binary hashes and complete compressed native receipts in this folder.

This qualifies bounded preparation, exact source playback and visible failure
handling on the recorded ARM64 machine. **Full R2 remains open.** Dense camera
playback still fails, and dense six-channel edit input misses its existing limit.
All final native commands exited successfully; their individual gate failures
remain explicit in the JSON. Command exit is not a blanket performance pass.

## Final native observations

| Observation | Real camera, 1280×720 / 24 | Six-channel fixture, 320×180 / 24000/1001 |
| --- | ---: | ---: |
| Actual source-sequence draws by observed sound end | 314 / 314 | 79 / 79 |
| Missing / invalid / overflow receipts | 0 / 0 / 0 | 0 / 0 / 0 |
| Selection-to-GPU-completion p95 / maximum | 7.051 / 15.269 ms | 10.380 / 11.988 ms |
| Maximum queued pictures | 8 | 8 |
| Sampled combined app/worker RSS peak | 516,320 KiB | 253,680 KiB |
| Cancel stopped codec during initial preparation | 128.447 ms | 121.669 ms |
| Kill playing picture codec and retire it | 56.592 ms | 47.865 ms |
| Stopped picture codec → visible underflow/retirement | 482.916 ms | 481.462 ms |
| Full native workflow input p95, gate 50 ms | 19.192 ms | 18.626 ms |
| Cached paused draw p95, gate 250 ms | 18.674 ms | 18.315 ms |
| Dense 128-cut input p95, gate 50 ms | 44.812 ms | **57.501 ms: fail** |
| Four dense sequence seeks, p95 request-to-draw, gate 250 ms | **300.185 ms: fail** | 63.661 ms |
| Dense cut reaches exact sound end | **No: picture underflow** | Yes |

The first prepared picture completed its GPU draw before sound started. The
numbered per-play receipts agree with bounded app counters; no diagnostic records
were lost. Exact sample-position resume, retry, playing seek, natural tails,
end/restart, document edit cancellation, save, recovery, reopen and source
preservation pass in the separate native workflow receipts. Picture, PCM and
device children were actually stopped/killed and reaped. No mock process result
stands in for those trials.

RSS is sampled every 50 ms, can count shared pages twice and excludes unreported
GPU allocations. GPU textures retain the existing graph's separate live budget.
Preparation-request-to-draw includes intentional future buffering; it must not be
confused with the separate selection-to-draw measure above. Completion before the
observed final sound boundary does not establish individual frame deadlines or
physical display/audibility/drift acceptance.

## Editing and delivered media after failure

The dense camera sequence stops visibly while showing frame 13. The editor remains
usable: Remove/Undo/Redo/Undo, native Save, independent recovery, native master
selection/export, reopen and 800×600 viewing complete. The dense six-channel run
also completes these paths and reaches its exact sound end, but its Redo input
is 57.501 ms against the unchanged 50 ms gate. Both reports say `qualified:false`.

Both full masters remain byte-identical to the independently slice-verified
[sound-index masters](../r2-sound-index/README.md):

- Camera: `2b998a7824cf19b7c88aeb5ee07388819521ccd9f3ff9842605e7fe437404a72`.
- Six-channel: `77955d21f2bf4698d820f88fe067714c844db1449d04a5347cf08ac61c6a04aa`.

Fresh independent FFmpeg decode agrees with the current shared-renderer receipt
over all 471,859,200 / 29,491,200 picture bytes and 1,024,000 / 6,150,144 PCM bytes.
The earlier independent source-slice comparison is reused through exact whole-file
identity; that source-slice program was not rerun. Originals, sources and saved
graph content remain unchanged.

## Retained pilots and verifier corrections

The instrumented baseline `c8d5468` and first prepared pilot `036e6d6` each recorded
all 314 camera and 79 six-channel draws. A later `3055a7c` camera run concurrent
with a build retained one actual omission, frame 10, and one discarded queued
picture. Its complete 313/314 receipt is retained; snapshots alone no longer hide
this difference. The concurrent build is context, not proof of the omission's cause.

The first fault driver requested already-selected frame 0 and incorrectly waited
for a new changed-state receipt. Its trace is retained; the corrected driver uses
the current owned completed picture. An early dense-cut driver required sound end
and aborted after the actual underflow. The next driver continued persistence but
assumed a retired viewer still had an active task sequence; it now checks the
captured playback session/sequence. Both failed traces are retained.

An earlier six-channel scrub trial delivered the correct latest frame but counted
no UI rejection: the single result slot can replace an obsolete result before UI
polling. Final diagnostics separate these replacements from UI rejections. Both
final native scrub trials observe rejection of an obsolete result and correct
latest-frame display. Earlier failed observations are preserved in `pilots/`.

These corrections change the verifier's treatment of real outcomes; they do not
waive the dense playback or input failures. The final driver continues independent
cases and records every process exit in `native/exits.tsv`.

## Local checks and provenance

`latest-checks/` qualifies `5578d56`: 209 default + 7 doc tests, 210 all-feature + 7
doc tests, formatting and workspace/all-target/all-feature Clippy all pass.
`checks/` preserves the earlier `a69f768` check run. All tests, builds and outputs remain
under the project. The shared Cargo target is reused explicitly; frozen executable
hashes and tracked source-input hashes identify actual tested artifacts.

Hardware-matrix, sustained 1080p/4K, warm/cold seek, two-hour physical drift, client
approval and independent-user gates remain open. Dense random-access preparation
and the existing edit-input miss continue as R2 work; no release gate is relaxed.
