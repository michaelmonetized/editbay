# Bounded forward picture decoding

Issue [#36](https://github.com/michaelmonetized/editbay/issues/36) is published in
[PR #39](https://github.com/michaelmonetized/editbay/pull/39), following
[PR #37](https://github.com/michaelmonetized/editbay/pull/37). Production, lab and
software qualification use **7802f7aa7df134aa3d6bec21f99fc04becdbeba9**. Frozen
app/CLI/lab, compiled-input, source, project and lockfile hashes are retained.
Local native trials ran **2026-10-06 05:56:02–05:57:54 UTC**, after our builds ended,
on the recorded Apple M1 Pro Vulkan/Honeykrisp host. A separate local project
compiled during software checks; a timestamped memory-pressure sample during
native trials is retained. No other process was stopped for these measurements.

## Exact decoding and cleanup

**198 default + 7 doc tests**, **199 all-feature + 7 doc tests**, formatting and
workspace/all-target/all-feature Clippy pass. Actual variable-rate H.264 B-frame
tests compare every returned pixel against independent FFmpeg output. Eight
skipped pictures use forward advancement; nine use seek. Reverse/EOF, cached-hit
decoder state, eviction, changed intermediate timestamps, held pins and failure
cleanup pass. Intermediate pictures produce no RGBA output allocation/conversion.

The packaged **app** worker handles the camera; the **CLI** worker handles the
six-channel AAC fixture's pictures. Each uncached qualification makes ten exact
requests: one sequential, five forward and four seeks. Twenty skipped source
timestamps are validated; every returned hash/color/alpha/tick agrees with full
independent sequential decode. Parent mapped bytes/handles return to zero after
each unretained result. Clear reaps the process and clears sources, decoders and
retained payloads. Worker death is visible, retry reuses verified matching content,
and old receipts are rejected.

| Route | Already-decoded IPC p95, ms | Active request cancellation, ms | Stopped-child cancellation, ms |
| --- | ---: | ---: | ---: |
| Camera / app | 0.106167 | 4.407225 | 102.133398 |
| Six-channel / CLI | 0.406669 | 5.009521 | 105.300773 |

IPC remains below 10 ms p95; cancellation remains below two seconds. Active
cancellation targets an outstanding first-binding/uncached IPC request; it does
not separately instrument the child's precise native phase. Forward steps have
no new elapsed-time gate: the camera's eight-picture advance takes **50.329 ms**.
This bounded work is not a guarantee that arbitrary gaps fit a frame deadline.

## Complete native source playback

The same driver observes the frozen parent and new app, each from a fresh native
window. Source/project hashes are preserved, sound reaches its exact sample end,
last pictures complete, and the app plus observed owned workers are reaped.

| Source / app | Sequence frames | Completed-draw observations | Request-to-GPU p50 / p95 / max, ms | Sampled combined RSS peak, KiB |
| --- | ---: | ---: | --- | ---: |
| Camera / parent | 314 | 313 | 16.972 / 18.781 / 34.384 | 515,968 |
| Camera / new | 314 | 313 | 17.026 / 24.048 / 33.582 | 518,272 |
| Six-channel / parent | 79 | 77 | 17.561 / 27.276 / 35.435 | 199,872 |
| Six-channel / new | 79 | 79 | 17.613 / 26.851 / 28.296 | 237,712 |

These normal source runs use exact-next decoding throughout, so they do **not**
demonstrate a speedup from the new forward route. The camera is 1280x720 at 24 fps;
six-channel pictures are 320x180, with 44 actual source pictures and the preserved
natural sound tail. They do not qualify representative 1080p/4K or two-hour use.
Sampled RSS may count shared pages more than once and excludes unreported GPU
allocations; it is not complete peak native/device memory.

The viewer's existing `skipped_frames` counter is zero in these four runs, but
it counts **accepted worker pictures**, before their display callbacks complete.
The camera observations omit frame 1 in the parent and 87 in the new app; the
parent six-channel trace also has two absent observations. This may include
diagnostic sampling loss, so absent observations are not asserted to be actual
display drops. Neither zero accepted gaps nor these snapshots establishes zero
display drops. Actual bounded completion records and preparation ahead of the
sound clock continue in [issue #38](https://github.com/michaelmonetized/editbay/issues/38).

## Dense edits, masters and retained misses

Both 128-cut native workflows complete remove/undo/redo/undo, full sound, save,
independent recovery, reopen, chooser export and the 800x600 viewer. All picture
and original-channel PCM bytes pass fresh independent decode against the shared
renderer receipts. Both whole MOV hashes equal the independently source-slice
qualified parent files. The earlier source-slice computation was not repeated.

Camera input p95/max is **44.386 ms**. Six-channel Remove reaches **53.123 ms**,
missing the unchanged 50 ms input gate; its receipt remains `qualified: false`.
The other six-channel inputs are 35.534, 45.464 and 34.147 ms. No repeat is used to
erase that miss. Camera seek maximum/p95 is **333.848 ms**, also above the existing
250 ms warm target. Six-channel seek p95 is 54.561 ms.

The camera records **70 accepted-picture gaps** in the dense cut; six-channel
records zero. Neither dense workload uses the bounded forward route because its
uncached requests fall outside the eight-picture bound. Random-cut decode/cache
work remains open. Native viewing uses zero CPU readbacks. The completed camera
source and recovered six-channel compact screenshots were inspected.

`qualification.status` says the scripted trials completed, not that every gate
passed. `gates.json` retains the input/seek/measurement limits and full R2 as open.
No physical audibility, two-hour drift, other GPU family, client approval or
independent-user acceptance is inferred.

Readable receipts omit repeated control rectangles, clip arrays and per-frame
observation arrays. Compressed complete receipts and raw native/focus traces
retain those observations; saved/recovered project manifests are also compressed.
Original editable manifests and full media remain in `artifacts/forward-pictures`.
