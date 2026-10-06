# Procedural mask qualification

Issue [#45](https://github.com/michaelmonetized/editbay/issues/45), after
[PR #44](https://github.com/michaelmonetized/editbay/pull/44).
Frozen runtime `0ee08f7` is identified by source, lockfile and binary SHA-256 in
[final/](final/). The [mask contract](../../PROCEDURAL_MASKS.md) defines geometry,
coverage, types, cache identities and resource bounds.

## Actual source and delivery results

The real 1280×720 camera project has 314 frames and mono sound. The 320×180
six-channel AAC project has 79 composition frames, including its natural sound
tail after source pictures end. Both animated-mask qualifications pass their
declared fixture pixel and incremental-GPU timing gates.

| Observation | Camera | Six-channel AAC |
| --- | ---: | ---: |
| Maximum FP16 mask/picture error | 0.000398044 | 0.000476202 |
| Maximum FP32 mask/picture error | 0.000007023 | 0.000003607 |
| Incremental FP16 GPU p95, three frames | 3.628 ms | 1.324 ms |
| Incremental FP32 GPU p95, three frames | 12.851 ms | 1.406 ms |
| Decoded master maximum channel-byte error | 1 | 1 |
| Lab observed RSS high-water | 177792 KiB | 107376 KiB |
| Supervised delivery-worker observed high-water | 368064 KiB | 130064 KiB |

The engine command authors editable geometry and animated feather through normal
commands, verifies undo/redo/save/recovery/reopen, renders both precisions, and
delivers a real master. Independent f64 mask/compositing math consumes unmasked
source-color GPU pixels; source color evaluation is deliberately shared. The
synthetic pixel tests use fully independent solid inputs. Installed FFmpeg
decodes actual masters for reference-byte and PCM checks. Full original-channel
PCM hashes match the earlier unmasked masters exactly for both sources.

Timing excludes the preceding source-color warmup, source verification/rebind and
readback; it includes mask/Over graph evaluation, submission and completed GPU
work. Three selected frames per precision are scoped samples, not a sustained
performance claim. Lab and worker high-water observations are separate components,
not a complete simultaneous process-tree/driver-memory bound. Full 4 GiB baseline,
large-workload and hardware qualification remain open.

## Native compact cut workflow

Actual 1440×900 and 800×600 windows show the mask. The compact image rectangle is
170.656×96 pixels and the full rectangle is visible; source marking and cut
creation remain accessible beside it in the scrolling edit panel. Screenshots
were inspected, including the completed masked cut, not just a preparing state.

Both runs mark source frames `[1,7)`, create a six-frame linked cut, undo, redo,
save, explicitly view that cut, choose a real output path in the native chooser,
export and reopen the saved document. Installed FFmpeg compares the delivered
cut against the exact interval from the independently checked uncut mask master.
**Every picture byte and original-channel PCM byte matches** for camera and
six-channel sound. Mask geometry, animation and original source references survive
the native edit and persistence workflow.

Camera/six-channel create-input latency is **38.123/26.659 ms**. Ten measured
native seek/draw completions each have p95 **216.351/31.307 ms**. These pass the
50 ms input and 250 ms seek gates. Initial renderer startup is not included in
those seek samples. Native traces, chooser/export receipts, exact decoded hashes,
saved projects and inspected captures are in `final/native-camera/` and
`final/native-six/`.

## Validation and retained failures

The six actual GPU mask tests cover even-odd/self-intersecting/degenerate geometry,
edge feather, inversion, transparent-background compositing, inactive geometry and
masks, animated/reverse nested time, cumulative work limits, changed cache inputs,
stale/foreign receipts, cancellation with submitted graph work and consumer pins.
Unsupported asset masks fail before dispatch. Cancellation clears pending GPU
ownership within two seconds while a retained output stays charged.

The earlier camera/six-channel engine receipts remain in `earlier/`. Their original
`024b46a` source was restacked as `15d5b96`; recorded `crates` tree hashes are equal.
The initial compact layout exposes only 47.5 pixels of a 96-pixel image and fails
its visible-image gate. The next candidate exposes offscreen/wrapped mark controls.
A later camera run misses a requested input/draw acknowledgment; six-channel
automation correctly rejects its missing mark-out control. Their screenshots,
logs, source identities and full traces remain preserved. Final controls use
separate source/marking rows, current visible geometry and observed keyboard
focus. These failures have not been relabeled as passes.

Local **226 default tests + 7 doc tests**, **227 all-feature tests + 7 doc tests**,
fmt and workspace/all-target/all-feature Clippy with warnings denied all pass on
2026-10-06. Full logs and explicit exits are stored under `final/checks/`.
Native tests and packaging stay local.
No physical display timing, speaker audibility, client approval, independent-user
acceptance, R4/R5 artist workflow or complete R2–R11 release is claimed. The parent
sustained playback gate still fails and remains open on #43 / PR #44.
