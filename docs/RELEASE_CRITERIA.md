# Release criteria

The full suite is ready only when every roadmap gate below has passing evidence.
An implemented issue, a passing test suite, and a qualified release are separate
states. A narrower release names its supported jobs and keeps all other gates open.

Budgets are set before qualification. Changing a budget requires a dated reason
and a new measurement; a slow run does not change the target automatically.
Measurements record the source hash, project revision, application commit,
lockfile hash, machine, driver, backend, workload, and sample count. Report p50,
p95, maximum, failures, and peak resident/device memory. Synthetic fixtures test
invariants; permissioned client jobs qualify production quality.

## Gates

| Gate | Required evidence | Pass criterion |
| --- | --- | --- |
| R0 persistence | CLI and core create/edit/save/checkpoint/recover; corrupt/truncated files; concurrent writers; process interruption | Zero overwritten originals, invalid published files, lost acknowledged saves, or stale writes; independent recovery identity; older valid checkpoints remain discoverable |
| R0 engine | Native decode, audio callback clock, wgpu FP16/FP32 composition, cancellable background export | Real decoded frames and samples; output reopens; color agreement within 0.002 FP16 and 0.00002 FP32; measured timing/memory/copies/backend; cancellation ends underlying work within 2 s and publishes no partial destination |
| R0 inference | Approved pinned SAM-family and human-matting artifacts; native preprocessing, propagation, cancellation and reference comparison | Artifact/code/license hashes recorded; reference mask IoU >= 0.99 and alpha mean absolute error <= 0.001 on identical inputs; recurrent resume error <= 0.0001; no production Python; actual hardware results and unsupported routes named |
| R0 reference/automation | Permissioned job inventory, immutable fixture hashes, conversion reports, command/MCP contract | Each claimed workflow has an input, expected output and ownership record; conversion preserves source bytes and refuses destination overwrite; every advertised mutation requires expected revision and normal command/undo validation |
| R1 local workspace | Native welcome/catalog, tabs, saves, process kill/restart, untitled/inactive recovery, small-window and palette captures | 100/100 recovery trials preserve latest acknowledged checkpoint; dirty documents checkpoint after 1 s idle or within 10 s maximum; input p95 <= 50 ms while scanning/saving; no obsolete worker publication; useful layout at 800x600 and 1440x900; no login required |
| R2 engine | 1080p and 4K long playback/seek/export/cache measurements on supported Intel, AMD, NVIDIA and ARM64 | 1080p30: dropped frames <= 0.1%, frame p95 <= 33.3 ms; 4K30 on qualified hardware: <= 1% drops; seek p95 <= 250 ms warm / 1 s cold; audio drift <= 20 ms over 2 h; bounded memory <= 4 GiB for the declared baseline; cancellation <= 2 s; preview/export same evaluation |
| R3 edit | Permissioned commercial, social and interview jobs with client revisions, archive/reopen and FCPXML/Premiere XML/OTIO migrations | All three delivered and accepted; exact integer edit boundaries, sync and identities survive undo/reopen; zero undisclosed conversion loss; sources retain hashes; output metadata, duration and channels match delivery profile |
| R4 isolation/repair | Identical hard-shot set with hair, blur, cuts, occlusion and retiming; raw/matted/refined comparisons and artist corrections | Cached preview/export alpha agrees within 0.0001; resumed processing agrees within 0.0001; no stale correction overwrite; all unresolved ranges visible; quality and correction time meet independently approved job target; vision controller enabled only with measured improvement |
| R5 motion/compositing | Editable brand film, kinetic type, tracked graphic and multi-source composite, followed by requested revisions | All four delivered without flattening required editable structure; packaged fonts/assets resolve offline; preview/export timing and graph agree; external artwork changes require explicit version choice |
| R6 interview/event | Multicam event and transcript-edited interview, captions, retimes and horizontal/vertical variants | All variants accepted; sound/picture drift <= 20 ms; caption boundaries within one sequence frame of approved timing; source-frame mapping survives revision and supported interchange |
| R7 color | Independent SDR/HDR transform comparison, scopes, reconform, exports and qualified monitor/I/O | FP32 transform agreement <= 0.00002 for defined test vectors; file range, primaries, transfer and mastering metadata match profile; calibration evidence exists; actual grade/reconform accepted |
| R8 sound | Interview, dialogue and multichannel jobs; recording/device trials; stems and external handoff | Sample-accurate alignment, channel order and latency compensation; zero callback allocation/blocking in qualified route; no xruns during 2 h workload; loudness within 0.5 LU and true peak within 0.2 dB of independent meter/profile; stems/handoff accepted |
| R9 clients/teams | Two distinct accounts, owner/editor/reviewer, large media, interrupted transfer, review/revision/revocation/restore/offline | 10 GiB transfer resumes with matching hash and no duplicate revision; every role-denied operation fails; revocation blocks next request; annotations retain exact version/frame; restored local copy preserves original; local work survives network loss |
| R10 3D/rigging | Human reconstruction/correction, temporal retargeting, tracked 3D composite and external handoff | Editable skeleton/skin/curves/passes survive save/reopen; coordinate/time alignment meets declared fixture tolerances; artist approves motion/contacts and composite; native artifact/terms evidence exists |
| R11 installation/delivery | Signed/versioned dependencies, local package hashes, clean install/update/rollback, format/plugin/device matrix, interruption/QC | Correct architecture and checksums on every advertised artifact; clean-machine launch works; update/rollback preserve projects/preferences; failed jobs preserve sources and remove unpublished output; every supported format/device has actual qualification evidence |
| GTM repeat use | Independent users across all five professional roles, accepted first and second jobs, migration/help/compatibility pages | At least 3 independent users per advertised role complete 2 paying or equivalent production jobs each; client revisions and handoffs accepted; zero unresolved data-loss/security defects; supported matrix and limitations published; consented activation/repeat-use receipts exist |

## Enterprise requirements

Recovery, schema compatibility, deterministic command revisions, portable offline
assets, bounded jobs and visible errors apply to every release. Cloud releases
also require tested authorization for every role, revocation, immutable versions,
resumable checksummed transfer, credential protection, and exportable customer
data. Local opening, correction and export never depend on subscription, login,
network availability or a model download.

Native qualification runs locally with `cargo test --workspace --locked`,
`cargo fmt --all -- --check` and
`cargo clippy --workspace --all-targets --locked -- -D warnings`.
Hardware, native visual, live-service and independent-user receipts are recorded
separately. Passing software checks cannot close those gates.

## Evidence and current disposition

Evidence summaries live under `docs/evidence/`; large outputs and private media
stay outside Git. A receipt includes the exact command, timestamps, exit status,
metrics and hashes. PRs identify the completed issue(s), parent PR and open gates.
R0's persistence, documented ARM64 engine/inference feasibility and scoped
reference/automation gates pass. Interchange converters and completed production
jobs are not advertised by this milestone. R1's local native workspace passes its
100-trial recovery, input, timing, storage-error and inspected layout gates; see
[R1 evidence](evidence/r1-workspace/README.md). The recovery scheduler starts after
1 s idle; the timing receipt allows at most 250 ms for worker preparation and
durable publication, and retains the 10 s continuous-edit maximum.
R2's typed document, migration and frame-plan slice passes its software checks
and a fresh 100-trial native workspace regression; see
[R2 document evidence](evidence/r2-document/README.md). Its production picture,
sound, seek, cache and long-playback gates remain open.
R2 native stream ingest passes real source/worker/window/save/recovery checks;
see [ingest evidence](evidence/r2-media/README.md). Uncached CPU seek p95 is
631.905 ms for the portrait source and 469.515 ms for the camera source, above the
unchanged 250 ms warm target. Exact pixels and native sound samples pass; cache,
shared rendering, playback/drift, image sequences and the hardware matrix remain
open. Import cancellation through the actual window measures 126.238 ms.
A fresh native 100/100 recovery regression on the ingest artifact retains
originals/checkpoints and independent identities; input p95 is 26.730 ms and
UI checkpoint publication p95 is 0.928 ms. Incomplete driver attempts are
recorded separately and do not count as passing trials.
Other hardware, production model packs and full workflows remain R2–R11 gates.
See `IMPLEMENTATION_STATUS.md` for implementation and measurement receipts.
