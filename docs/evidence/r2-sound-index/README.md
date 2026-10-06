# Bounded sound planning for longer cuts

Issue [#32](https://github.com/michaelmonetized/editbay/issues/32) follows device
containment [PR #33](https://github.com/michaelmonetized/editbay/pull/33).
The sound runtime is **7dd6a8c**; the complete app/CLI/lab build and software tests
are **345f9d9**. Final native driver **064c85d** only waits for the replacement
preview request after Undo; production app/worker binaries are unchanged.
Exact executable, compiled-input, lockfile, source and reference hashes, commands,
compiler/host identity and logs are retained alongside the receipts.

Trials ran locally on the recorded ARM64/Omarchy host, **2026-10-06 04:51:46–04:59:10
UTC**, with a stopped native-driver attempt and a recorded restart at 04:54:22.
Timing trials ran without our own builds. All work and temporary files stayed
under the project. Full media remains in `artifacts/sound-index/`; original media
was preserved.

## Sound and retained storage

The existing 5 ms p95 gate is unchanged. Every final sound-planning workload passes:

| Workload | Blocks | Planning p50 / p95 / max, ms | Compiled paths | Compile, ms | Process high water at compilation, KiB |
| --- | ---: | --- | ---: | ---: | ---: |
| Camera, 128 one-frame cuts | 63 | 0.875 / 1.867 / 3.014 | 128 | 2.245 | 43,952 |
| Six-channel, 128 one-frame cuts | 63 | 0.661 / 2.772 / 3.270 | 128 | 2.195 | 45,264 |
| Six-channel, 256 one-frame cuts | 126 | 0.689 / 2.281 / 3.152 | 256 | 4.478 | 50,592 |

Each block selects at most four conservative candidate paths and evaluates at
most 4,096 positions, with at most 19 interval-tree visits. Allocated slots include
silence and remain separately charged. Compiled process high water is below the
predeclared 256 MiB gate; this is not the native editor's total GPU/process memory.
Every original-channel float sample agrees exactly with independently decoded
source-master slices. Undo/redo, save, recovery and source identities also pass.

Actual F32 stereo device callbacks at 44.1 kHz reach **235,200 / 235,436 / 470,871**
samples exactly. Prepared queues never exceed 16,384 frames; device and PCM
processes are reaped. The deliberately loud six-channel fixture clips 24,927 and
50,590 monitor samples in the 128/256-cut trials. Original-channel float masters
preserve that headroom and remain exact.

The original camera/six-channel PCM-worker qualification also passes every gate:
planning p95 **2.158 / 1.106 ms**, zero PCM error, foreign/stale rejection, held-pin
accounting, source preservation and cancellation cleanup. Six actual stream
cancellation/preparation-cancellation/device-death trials pass, with maximum
complete retirement **20.277 ms**, below two seconds.

## Native editing and whole masters

Both native 128-cut projects pass random seeks, remove/undo/redo/undo, full sound
playback, save, separate recovery, chooser export, recovered-project reopening
and the 800x600 viewer. All linked clip/source identities survive. Eight edit
inputs have p95/max **44.814 ms** for camera and **43.861 ms** for six-channel,
under the unchanged 50 ms gate. Full-size and compact screenshots were inspected.

Independent FFmpeg source-slice comparison verifies every delivered byte:

| Master | RGBA bytes | Original-channel PCM bytes |
| --- | ---: | ---: |
| Camera, 128 pictures | 471,859,200 | 1,024,000 |
| Six-channel, 128 pictures | 29,491,200 | 6,150,144 |

Receipts retain per-cut source/record boundaries and hashes, complete stream
hashes, metadata and file hashes. Native viewing uses no normal CPU pixel readback.
Saved/recovered manifests and raw native/focus traces are retained as gzip files,
alongside screenshots. Readable native JSON omits repeated control rectangles and
reports counts for repeated clip lists; compressed observation files preserve the
original complete receipts. Original editable manifests remain in the worktree's
`artifacts/sound-index/qualified-native-{camera,six}` directories.

## Remaining engine limit

These are sound and functional editing/delivery gates, not sustained picture
playback acceptance. The camera viewer skips **98** frames between completed
pictures; it displays frame **118** when sound reaches record frame **127**.
That observation records **3,194 GPU dispatches / 3,158 cache evictions**. The
smaller six-channel picture fixture skips zero frames but records 8,976 dispatches.
The shared renderer still dispatches inactive compositing work. This measured
next dependency is [issue #34](https://github.com/michaelmonetized/editbay/issues/34).
`gates.json` leaves full R2 qualification false.

The canonical assembly still caps its single Mix node at 256 inputs. The index's
larger retained-path limits are separate; 1,024-path core tests and a 16,384-entry
lookup test prove mathematical/storage behavior, not a rendered 16,384-cut job.
These short, dense edits do not establish long-duration hardware stability,
1080p/4K performance across GPU families, physical audibility, two-hour drift,
client approval or independent-user acceptance. Those roadmap gates remain open.

## Software and earlier measurements

**194 default + 7 doc tests**, **195 all-feature + 7 doc tests**, formatting and
all-target/all-feature Clippy pass. The lab-only final driver fix also passes
all-target/all-feature Clippy and the actual complete native trials. Differential
tests compare every sample plan from the rational shortcut with equivalent
piecewise paths through the full evaluator, including reverse playback, fractional
rates, static gain and four output rates. Existing nested/gain/freeze/boundary,
ownership, cancellation and native worker tests continue to pass.

`earlier/` preserves the first 5.950 ms pilot miss, the 7.545 ms cropped-path miss,
the accidentally copied old executable's explicitly attributed repeat, the first
successful rational-step trials, and the failed after-Undo native driver trace.
The parent PR #33 camera miss also remains intact. Passing final measurements do
not erase those earlier observations or change their predeclared gates.
