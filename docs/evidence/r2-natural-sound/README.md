# Naturally timed source sound

This is the source-authoring dependency of R2, tracked by
[issue #22](https://github.com/michaelmonetized/editbay/issues/22) and
[PR #25](https://github.com/michaelmonetized/editbay/pull/25), stacked on #23.
The [native contract](../../NATIVE_PREVIEW.md) describes explicit sound selection,
exact nested timing, reciprocal links and one-group document ownership.

Local Linux ARM64 / Apple M1 Pro / Omarchy. The final implementation is
`7798f24` (full identity in `source-commit.txt`); compiled input hashes,
lockfile/artifact identities and complete test logs accompany this receipt.
Builds and temporary files stayed in project storage. The reused build target
was rebuilt locally with two jobs, and binaries were copied into the ignored
`artifacts/natural-sound/qualified-bin/` before native qualification.

Default workspace tests: **154 plus seven documentation examples**.
All features: **155 plus seven**. Zero failed or ignored. All-target/all-feature
Clippy with `-D warnings` and formatting pass. All-feature checks use the retained
project-local libtorch. `checks.json` and the complete logs record each result.

Core tests compare every exact sample center across 24/25/30, 30000/1001 and
24000/1001 fps, 44.1/48 kHz, delayed/early sound and fractional tails. Wrong
streams, absent overlap and clock overflow fail before mutation. A single undo
group, saved document, checkpoint and independent recovery preserve the graph
and original sources. Twenty real MOV cases compare every rendered sample to a
separate FFmpeg decode: stereo float PCM and six-channel AAC, delays, preroll,
all five picture rates, unchanged channel identities, **zero PCM error**.

The Rust native driver imports original stream metadata, then uses actual window
controls to create the linked sequence, step/scrub, kill/retry/cancel the codec,
Undo/Redo, Save, kill/restart, recover independently and reopen the saved original.
It renders every saved and recovered sound sample through the packaged app's
PCM worker, compares each to independent PCM, and presents the recovered last
picture frame. Saved/recovered sound hashes must agree. Original source, saved
project and checkpoint hashes remain unchanged by recovery. Import here is a
native Rust seed; separate UI-ingest receipts remain under the earlier ingest
and native-preview evidence.

Final native receipts and measurements follow in `camera.json` and
`six-channel.json`. Their gates remain input p95 <=50 ms, cached submitted GPU
draw p95 <=250 ms and active cancellation <=2 s. Sound preparation and rendering
measurements use 4096-frame bounded blocks, with the final partial block retained.

Final qualification: **2026-10-06 00:38:55–00:39:30 UTC**.

| Native case | Input p95 | Cached draw p95 | Active cancel | Saved sound preparation / render p95 |
| --- | ---: | ---: | ---: | ---: |
| Camera FP32 | 19.274 ms | 19.106 ms | 121.967 ms | 3.680168 / 1.058679 ms |
| Delayed six-channel FP16 | 18.312 ms | 21.152 ms | 126.352 ms | 1.566810 / 0.851385 ms |

Every saved and recovered sample agrees exactly with independent PCM. Camera:
**627,040 original samples plus 960 silent tail samples**. Six-channel:
**151,600 original samples, 4,976 leading and 1,582 trailing silent samples**;
sound extends a 44-picture source to a 79-frame sequence. Recovered preparation
p95 <=2.207693 ms and render p95 <=1.348308 ms. All four sound passes retain the
existing **5 ms preparation / 20 ms unity rendering** bounds. Native parent HWM
is <=116,688 KiB; this does not establish the larger production-workload budget.

This is software input, GPU completion and exact offline sound evidence. It does
not establish device playback, physical audibility, long drift, shared delivery,
other hardware or the complete R2/enterprise/adoption gates.

## Earlier checks and the visible-error correction

`debug-native/` retains the earlier unoptimized camera receipt and its exact
artifact hashes. Its PCM comparison passes; its sound preparation timings are
debug-build measurements, not the optimized 5 ms sound-preparation qualification.
The first release six-channel fixture used MPEG-4 without declared BT.709
primaries/transfer. Native viewing refused it, as the color contract requires.
That run exposed a worker-exit race that could replace the specific error with
"Viewer worker stopped". The final code reads terminal events and thread state
under the same mailbox lock; a concurrent 256-iteration regression retains the
original error and detects genuinely empty exits. `earlier-fixture/` preserves
the failed native trace, command logs, stream metadata and artifact hashes.
The same negative case also exposed suppression of the original error after
renderer destruction cancelled its lifetime token. Errors now publish after
cleanup; explicitly cancelled views have already retired their event owner.
The actual negative native run in `visible-color-error/` now displays
"source matrix/range needs a supported native interpretation". Its recorded
unoptimized app uses the same final source; the driver intentionally exits
without a successful draw for this rejected fixture.

The final generated fixture uses H.264 with explicit frame and bitstream BT.709
declarations, 24000/1001 fps and delayed six-channel AAC. The original failed
fixture remains intact. No source was relabeled in the project or bypassed in
the renderer. Commands use the frozen binaries:

```sh
editbay-lab native-preview editbay-studio CAMERA NEW_DIRECTORY full
editbay-lab native-preview editbay-studio SIX_CHANNEL_MOV NEW_DIRECTORY half
editbay-lab natural-sound SOURCE editbay-studio
```

Native screenshots at 1440x900 and 800x600 document stream selection, normal
viewing, recovered content and the sound-only final picture region. Private
media, original binaries, projects and full native traces remain under ignored
`artifacts/natural-sound/`. Native streaming is the next dependency, issue #24.
