# Sound-device containment qualification

Issue [#30](https://github.com/michaelmonetized/editbay/issues/30) is published in
[PR #33](https://github.com/michaelmonetized/editbay/pull/33), stacked on linked
native editing [PR #31](https://github.com/michaelmonetized/editbay/pull/31).
Production runtime source is **f8eecdf**; the final lab driver is **31f4e41**.
The latter changes only native qualification seeking and complete retirement
measurement. Compiled-input hashes, exact frozen binary hashes, local build logs,
compiler/host identity and source commits are retained here.

Final combined trials ran **2026-10-06 03:56:35–03:58:00 UTC**, on the recorded
ARM64/Omarchy host, without own builds during measurement. Project state, temporary
files and full MOV masters stayed under the worktree. The subsequent dedicated
camera PCM repeat used the identical frozen lab binary.

## Device and process outcomes

All **16** streaming trials pass: full playback, ordinary/preparation cancellation,
PCM death/starvation, device death/stall and cancellation of a frozen device, for
both camera and delayed six-channel AAC sources. The callback/renderer code is
unchanged inside the device child. Actual default-device F32 stereo at 44.1 kHz
reaches **576,975** camera samples and **145,308** six-channel samples exactly,
with **2,453 / 617** callbacks. Prepared sound never exceeds 16,384 frames.
The deliberately loud six-channel monitor clips 15,585 samples; original-channel
float delivery retains its headroom and is independently exact.

Maximum stream interruption/complete retirement is **613.281 ms**, under the
predeclared 2-second gate. Both recorded process generations are gone, including
explicit collection of any PCM descendant adopted by the lab subreaper. These are
actual SIGKILL/SIGSTOP and cancellation trials, not mocked backend statuses.

All **24** actual packaged-worker protocol trials reject malformed/unknown-field,
out-of-order, stale, foreign-version/session, duplicate-start and descriptor-bearing
requests. Every worker is reaped. Separate **six** controller INT/TERM/KILL trials
terminate and reap both device and PCM descendants within **6.807 ms**. The lab
records only its own descendants; production does not install a global reaper.

## Native continuation after failure

Both actual windows survive device death, stall and cancel-stall, show failure or
stopped state, and retry with fresh device/PCM processes. The complete native
retirement maxima are **623.830 ms** (camera) and **618.009 ms** (six-channel),
including descendant reaping. Both proceed through source marking, linked cut
creation, append, split, remove, ripple trim, undo/redo, actual playback/pause,
save, independent recovery, chooser export and recovered-project reopening.
Screenshots of failure, reopening and the 800×600 view were inspected.

The two linked groups retain source [16,28) at record [0,12) and source [42,65) at
record [12,35). Each native run has five measured edit inputs: p95/max **26.498 ms**
(camera) and **27.182 ms** (six-channel), below the existing 50 ms gate.

The published masters independently match every selected source pixel and original
PCM byte: **129,024,000 RGBA / 280,000 PCM bytes** for camera; **8,064,000 RGBA /
1,681,680 PCM bytes** for six-channel. The camera master retains whole-file SHA256
`72162163cdf6efcc5065af22ba0b7a911a37e3837f83b53d57a7b9ffec9eb6a3` from the parent
qualification. Saved/recovered manifests retain identities and preserved media.
Large masters remain in `artifacts/device-worker/qualified-native-{camera,six}`.

## Performance limit retained

PCM comparisons have **zero error**, with ownership, cancellation and pin cleanup
checks passing. However, the combined run's camera block-planning p95 is
**10.698 ms**, above the unchanged **5 ms** gate. A dedicated repeat of the same
binary measures **2.521 ms**; six-channel measures **3.918 ms**. The miss is not
removed or reclassified. Its cause is not established, and a passing repeat does
not make all observations pass. `gates.json` explicitly leaves
`sound_planning_all_observations_5ms` false. Longer-edit indexing and planning
performance continue in [issue #32](https://github.com/michaelmonetized/editbay/issues/32).

Device containment and complete native continuation pass their scoped gates.
Full R2, sustained 1080p/4K across hardware, physical audibility, unplugged-device
behavior, two-hour hardware drift, client acceptance and independent-user adoption
remain open. No such proof is inferred from callbacks, software clock tests,
independent file decoding or local screenshots.

## Software checks and earlier attempts

Runtime f8eecdf passes **185 default + 7 doc tests**, **186 all-feature + 7 doc tests**,
formatting and all-target/all-feature Clippy. The final lab-only changes also pass
all-target/all-feature Clippy and are exercised by the actual native runs.

`earlier/` preserves the unoptimized six-channel underrun and a first release
starvation probe whose lab controller had not yet collected the adopted PCM exit
status. The final probe explicitly reaps that descendant under the same deadline.
It also retains two native driver failures caused by requesting the already
selected frame and expecting a new change-only draw event. The final driver reads
the actual playhead and makes a fresh seek. Those attempts are not passing native
workflow receipts. The original portal zero-RT-timeout reproduction remains in
[the parent evidence](../r3-timeline/earlier/).

Readable streaming receipts omit high-frequency samples; native summaries omit
repeated control rectangles. Corresponding `*-observations.json.gz` files retain
the original complete receipts. Raw native traces and focus observations are also
retained. No failed measurement is removed by this presentation change.
