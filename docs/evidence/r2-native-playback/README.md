# Bounded native playback

Issue [#24](https://github.com/michaelmonetized/editbay/issues/24) supplies native
Play/Pause, exact sample resume, frame seek, end/replay, device-clock picture
scheduling and supervised sound preparation. This layer follows natural sound
PR #25. See the [implementation contract](../../STREAMING_SOUND.md).

Qualified runtime source: `9daa7c5` (full identity in `source-commit.txt`). The
compiled input manifest, lockfile and frozen app/lab hashes accompany this file.
Linux ARM64 / Apple M1 Pro / Omarchy; native ALSA `default`, stereo F32 at
44,100 Hz. Backend-reported latency was zero; this is not a physical latency
measurement. Builds, fixtures and temporary files stayed in project storage.
The reused target was rebuilt locally with two jobs and binaries frozen under
`artifacts/streaming/qualified-bin/` before trials. No own builds ran during
native timing qualification.

Final qualification: **2026-10-06 01:28:16–01:30:19 UTC**. Default workspace:
**163 tests plus seven documentation examples**. All features: **164 plus seven**.
Zero failed or ignored. All-target/all-feature Clippy with `-D warnings` and
formatting pass; `checks.json` and complete logs retain commands and results.

## Transport and exact time

Nine additional regressions cover bounded concurrent queue wraparound, original
channel/sample order, partial EOF, malformed/empty output, nonfinite/stale input,
cancellation, latched underrun/device failure, explicit monitor routing/clipping,
reachable graph channel inference, and integer sample clocks. Clock fixtures
include latency spanning several final buffers, timestamp jitter and arithmetic
over two simulated hours at 44.1/48/96 kHz near the integer range limit. The
two-hour arithmetic test is not a two-hour hardware run.

Callback source inspection confirms fixed atomic operations, slice fills/copies,
sample conversion and monotonic time reads. The callback does not allocate,
deallocate, lock, access files, decode, resample or log. Native cancellation is
an atomic store/load with a compile-time lock-free requirement. This statement
does not cover CPAL/backend internals or certify operating-system scheduling.

The final device trial streamed a **45-second** quiet synthetic H.264/AAC source:
**8,437 callbacks**, exact endpoint **1,984,500 samples**, maximum prepared queue
**16,384 frames**, zero monitor clips and no underrun. Elapsed wall time including
startup/retirement was 45.164823 s; parent HWM **57,584 KiB**. It exceeds the old
30-second full-buffer feasibility limit without growing the prepared queue.
`device-full.json.gz` retains every observation; `device-full.json` summarizes it.
Separate actual cancellation,
SIGKILL and SIGSTOP trials retired in **5.085176 / 42.391541 / 421.035280 ms**;
all owned children were reaped. The stopped codec exhausts prepared sound, causes
a visible underrun and cancels the underlying process. A video-only source also
completed **374 callbacks / 88,200 silent samples** with a bounded queue.

## Actual window and preserved documents

The Rust driver imports original stream records, then uses real window controls
to create the linked sequence, step/scrub, kill/retry/cancel the picture worker,
Undo/Redo, Save, Play, Pause, Resume, seek while playing, kill/stall the sound
worker, retry, reach the exact end and replay from zero. It edits the project
while playing and confirms that the old PCM child is gone. It then checkpoints,
kills/restarts, recovers independently, displays the recovered last picture and
reopens the saved original. Saved/recovered sound is rendered through the actual
app's packaged PCM worker and compared sample-for-sample with independent decode.
This import is a native Rust seed; earlier UI ingest has separate receipts.

| Native case | Input p95 | Cached draw p95 | Picture cancel | Sound crash / underrun retirement |
| --- | ---: | ---: | ---: | ---: |
| Camera FP32 | 19.882 ms | 17.366 ms | 119.755 ms | 62.505 / 432.493 ms |
| Delayed six-channel FP16 | 18.494 ms | 18.319 ms | 122.711 ms | 56.718 / 426.443 ms |

Budgets stay **50 / 250 / 2000 ms**. `gates.json` verifies these bounds and the
unchanged sound preparation/render p95 bounds **5 / 20 ms**. Across both saved
and recovered cases, preparation p95 is <=2.981545 ms and unity rendering p95
<=1.686169 ms. Every original-channel sample matches exactly: PCM error **zero**.
Original source, saved project and checkpoint hashes remain unchanged by recovery.
Camera pauses/resumes at sample **34,616**; six-channel at **31,852**. End/replay
reaches **576,975 / 145,308 device samples**, then starts at zero. Normal preview
performs zero CPU picture readbacks. The short advancing-picture observations
show zero skipped frames; they do not establish the sustained drop-rate gate.

The six-channel synthetic source deliberately exercises headroom: the stereo
listening matrix clips and the window displays **Monitor clipping**. Original
channel PCM remains unchanged. Camera and quiet-device trials do not clip.
Native parent HWM is <=237,088 KiB; GPU/combined-process long-workload memory
qualification remains separate. Inspected images include playing, 800×600,
visible codec/underrun errors and the recovered transparent picture tail.

## Retained candidates and remaining gates

`earlier/` preserves the first failed driver attempt. Its frame field had already
accepted and drawn the target before Enter, while the driver incorrectly required
a later draw. The driver now observes from the first interaction. No application
gate was relaxed. The intermediate passing camera receipt predates end/replay
coverage; only the frozen final artifact supplies the qualification above.

These are actual local device submissions, software input, GPU completion and
offline PCM receipts. Physical audibility, speaker timing, two-hour hardware
drift, sustained 1080p/4K drops/seek/memory, other devices/GPUs, calibrated sound,
shared delivery and complete R2 acceptance remain open. Shared export continues
in [issue #26](https://github.com/michaelmonetized/editbay/issues/26), followed by
timeline authoring and the remaining R3–R11 workflows. No full-suite, client or
independent-user approval is claimed.
