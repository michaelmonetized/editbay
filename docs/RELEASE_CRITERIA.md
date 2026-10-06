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

The shared GPU renderer now reuses validated transparent compositing inputs.
[Picture-identity evidence](evidence/r2-active-pictures/README.md) proves fixed
FP16/FP32 pixels and bounded allocation ownership, plus native whole-master
equivalence. The camera viewer still skips pictures and exceeds some existing
seek timings; one input candidate also misses 50 ms. These failures remain
recorded. No R2 frame, seek, memory, drift or hardware gate is relaxed or closed
by the reduction in GPU dispatches.

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
The compiled temporal slice passes its separate 1 ms CPU planning budget on actual
314/11,471-picture indices and explicitly synthetic 500,000-timestamp stress.
It preserves the published integer inspector and covers exact fractional/reverse
boundaries and content invalidation; see [temporal evidence](evidence/r2-temporal/README.md).
This CPU planning qualification does not satisfy rendered preview/export, seek,
sound-block, presentation or drift gates. Complete production playback remains open.
The raw decoded-picture cache passes real pixel, hit-sharing, payload/pin budgets,
eviction, cleanup, version rebinding and active cancellation checks. Cache-hit p95
is 0.005/0.003417 ms for camera/portrait; uncached miss p95 remains 295.999/941.064 ms.
This measures CPU raw-picture access, not graph/GPU/native presentation latency;
see [cache evidence](evidence/r2-picture-cache/README.md). Full seek and playback
gates remain open, including native IPC, rendered output, audio and hardware.
Other hardware, production model packs and full workflows remain R2–R11 gates.

The shared SDR GPU picture subset passes its separate **completed-picture kernel**
p95 <=33.3 ms budget: camera FP16/FP32 12.051/12.878 ms; portrait 9.559/14.548 ms.
FP16 linear error <=0.002 and FP32 <=0.00002 pass on every channel of the 12 real
reference pictures. 4,000 hits share resident outputs without new upload/dispatch/
readback; 480 native pictures complete, with source binding separated from steady
timing. Active graph-native-seek cancellation/join/cleanup <=2 s passes at
17.750/9.984 ms. Texture/raw ownership and zero cleanup pass declared limits.
See [GPU evidence](evidence/r2-gpu/README.md). These ordinal-mapped headless cases
exclude native presentation, codec IPC, audio/clock/drop/drift, masks/HDR and codec
delivery. The final portrait FP32 maximum is 18.340 ms; an earlier local candidate reached
51.263 ms, recorded separately. Full playback/seek/export and
enterprise/GTM gates remain open; the prior 250 ms uncached seek failures persist.
See `IMPLEMENTATION_STATUS.md` for implementation and measurement receipts.

The isolated retained codec/plane dependency passes its separate predeclared
**already-decoded IPC p95 <=10 ms** budget: camera 1.734 ms, portrait 1.151 ms
over 1,000 handoffs each. Fifty indexed pixels match full independent sequential
decode. Active outstanding request cancellation/join/reap is 29.416/18.164 ms;
SIGSTOP fallback is 104.839/104.413 ms, within 2 s. Byte/handle consumer pins,
worker death/rebind, stale/foreign/malformed rejection and zero owned cleanup pass.
The same typed graph over these actual app/CLI children completes 480 pictures
with camera FP16/FP32 p95 21.625/23.983 ms and portrait 25.242/28.276 ms, within
33.3 ms. Twelve independent full-picture comparisons retain the established
color bounds, with no timed sequence readback. The initial portrait FP32 run
missed 33.3 ms and is retained as an earlier candidate, not a passing receipt.
See [process evidence](evidence/r2-picture-worker/README.md).
Current uncached source seek p95 is 254.661/324.550 ms, still above 250 ms. These
are source/kernel receipts, not native surface/playback/audio/export qualification.
The full R2 and enterprise/GTM release gates remain open.

The native shared-device picture workspace passes its separate paused-viewer
gates on camera/portrait at FP16/FP32: **160 inputs** with p95 <=33.257 ms, **152
cached draws** with request-to-GPU-completion p95 <=38.215 ms and active cancel
<=137.680 ms. Budgets remain 50 ms / 250 ms p95 / 2 s. The same frozen artifact
passes fresh actual UI ingest and **100/100** kill/recover/reopen trials; 250
regression inputs have p95 29.030 ms. The initial input candidate missed 50 ms
and remains recorded separately. Exact GPU surface color/alpha/orientation,
resource lifetimes, current native Flea recovery and source preservation pass;
see [native preview evidence](evidence/r2-native-preview/README.md).
These receipts qualify paused pictures and the preserved document foundation.
Full sustained playback, uncached seek, sound drift, shared delivery, devices and
enterprise/GTM gates remain open.

The bounded sound worker passes its separately predeclared gates on actual camera
audio and six-channel AAC: preparation p95 **2.929/4.081 ms** against 5 ms;
warmed unity render **0.458/0.504 ms** against 20 ms; sinc at 44.1 kHz
**24.984/27.684 ms** against 40 ms; first native render **56.220/5.218 ms**
against 250 ms. Qualified PCM matches independent native sequential samples
exactly. Active cancellation/cleanup takes **0.133/0.087 ms** against 2 s;
sound-only high-water memory is **50,592/56,976 KiB** against 256 MiB. Failed
preparation candidates remain recorded; no budget was raised. See
[sound block evidence](evidence/r2-sound/README.md). These receipts do not close
device callback, sound codec isolation, two-hour drift or shared delivery gates.

The isolated sound dependency passes its declared PCM handoff p95 <=10 ms gate
at <=0.087751 ms over 4000 actual shared app/CLI transfers. All camera/six-channel
raw and nested graph routes agree exactly with independent PCM. Preparation p95
<=2.465802 ms, unity render <=1.030712 ms and sinc <=28.541994 ms retain their
existing bounds; combined parent/child HWM <=90016 KiB is within 512 MiB.
Active cancellation/join/reap <=3.158221 ms and SIGSTOP fallback <=102.833721 ms
pass 2 s. Earlier sinc/preparation misses are retained separately. See
[isolated sound evidence](evidence/r2-sound-worker/README.md). This closes the
bounded PCM process route, while native streaming, natural source timing,
devices, long drift, shared delivery and full R2 remain separate open gates.

Natural source timing now has separate [native and PCM evidence](evidence/r2-natural-sound/README.md):
20 decoded timing fixtures and both saved/recovered camera and delayed six-channel
graphs retain every sample with zero error. Preparation p95 <=3.680168 ms and
unity rendering <=1.348308 ms preserve 5/20 ms bounds. Actual native input p95
<=19.274 ms, cached draw <=21.152 ms and cancellation <=126.352 ms pass their
50/250/2000 ms limits. This closes the source-sequence timing dependency; device
streaming, physical audibility, drift, shared delivery and full R2 stay open.

Native streaming has scoped [window/device evidence](evidence/r2-native-playback/README.md):
45 seconds and 8,437 actual callbacks with an exact sample endpoint and <=16,384
prepared frames. Camera/six-channel native Play/Pause/seek/end/replay, codec
death/underrun/retry and edit cancellation pass. Input/draw p95 <=19.882/18.319 ms,
worker retirement <=432.493 ms and zero saved/recovered original PCM error retain
their unchanged limits. This completes the bounded transport dependency;
physical audibility, two-hour hardware drift, sustained 1080p/4K and shared
delivery remain open R2/R8 gates.

The first shared master now has separate [native/media evidence](evidence/r2-shared-delivery/README.md).
Actual camera and delayed six-channel exports preserve exact graph picture/PCM
hashes through independent FFmpeg decode, native retry, save and recovered projects.
Eight viewer inputs have p95 <=19.548 ms; six were accepted during export. Native
cancellation/retirement <=126.322 ms and stopped-child retirement <=172.906 ms
retain 50/2000 ms gates. Existing destinations, changed sources, malformed/stale
work and abrupt controller death are exercised; no unpublished files survive.
This closes one declared SDR full-sequence master. Broader profiles/ranges/queues,
long workloads, hardware/drift and R3–R11 workflow/adoption gates stay open.

The first linked timeline layer has [native and independent media evidence](evidence/r3-timeline/README.md).
Camera and delayed six-channel edits preserve exact source/record boundaries,
linked identities, undo/redo, native save/recovery/reopen and exported picture/PCM
bytes. Reversed source order and six stale CLI replays pass. Ten actual native
edit inputs have p95 <=24.354 ms, below the existing 50 ms gate. Saved/recovered
cuts produce identical whole masters. This qualifies the declared one-pair,
matching-profile subset, not full EB-030/031 or the paying-edit gate. Broader
editing/interchange, production client acceptance, sustained hardware workloads
and R4–R11/adoption gates remain open. The reproduced host portal failure is
retained; supervised sound-device isolation continues as issue #30.

Sound-device isolation is a separate EB-023/025 dependency: actual backend death,
stall, cancellation and controller-death trials must retire both process layers
within two seconds, preserve the editor, and permit playback retry followed by
native edit/save/recovery/export. Typed protocol substitution checks supplement
those real process trials. This does not close physical audibility, device unplug,
two-hour drift or the full R2 hardware gates. See
[device containment](SOUND_DEVICE_WORKER.md).

The [device evidence](evidence/r2-sound-device/README.md) passes 16 real streaming,
24 protocol, six controller-death and two complete native failure/retry/edit/export
workflows. Native retirement stays below 624 ms and edit p95 below 28 ms; independent
pixel/PCM slices are exact. Camera planning includes a retained 10.698 ms p95 miss
against 5 ms (same-binary repeat 2.521 ms); six-channel p95 is 3.918 ms. This is scoped
device-containment acceptance, not a blanket sound-performance or R2 pass.

[Prepared-picture evidence](evidence/r2-picture-clock/README.md) records 314/314
camera and 79/79 multichannel source-sequence GPU draws by observed sound end,
bounded queues, actual stopped/killed codec retirement below 483 ms and exact
sample resume. Both dense masters remain identical through independent decode.
The dense camera sequence still underflows and its four-target seek p95 is
300.185 ms against 250 ms. Dense six-channel Redo input is 57.501 ms against 50 ms.
An earlier source run's actual missing frame and all failed verifier trials remain
retained. This closes no sustained playback, physical drift or hardware gate.
