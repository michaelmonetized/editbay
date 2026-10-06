# Native sound backend evidence

Issue #47 / [PR #48](https://github.com/michaelmonetized/editbay/pull/48) follows
PR #46. These are actual ARM64 native-backend measurements, not physical output,
client approval or independent-user evidence. The existing 20 ms clock,
16384-frame preparation and 4 GiB sampled RSS gates remain unchanged.

## Unpatched CPAL 0.18.2 candidate

`unpatched/` preserves runtime `faf9930`, binary/lockfile hashes, invocation,
machine, original media identities, saved compositions and full compressed traces.
Each `qualification.json` reports actual acceptance; exit zero alone does not.

| Workload | Result | Maximum backend / host drift | Actual callback frames |
| --- | --- | --- | --- |
| Six-second camera | Pass, exact endpoint and cleanup | 0.019917 / 0.067475 ms | 2048 |
| Six-second six-channel source | Pass, exact endpoint and cleanup | 0.018917 / 0.072669 ms | 2048 |
| Requested 180 seconds of repeated AAC | **Fail after 91.471896 seconds** | Accepted observations 1.667293 / 2.660514 ms | 2048 |
| Owned-device 300 ms STOP/CONT | Pass: visible continuity failure and cleanup | See raw receipt | 2048 |

The sustained failure retains 14336 prepared frames and its first visible reason:
backend timing continuity was lost. Accepted observations exclude the callback
that failed; this runtime did not capture that rejected clock. Peak combined RSS
is 126464 KiB, both source/project hashes remain unchanged, and owned children
retire. Host memory pressure is in each raw record. It accompanies the failure;
it does not identify its cause.

The callback thread `cpal_alsa_out` is observed at normal scheduling (policy 0,
priority 0). PipeWire's `data-loop.0` is observed at policy 2, priority 20.
The presence of CPAL priority support does not establish callback promotion.

## Snapshot candidate

The snapshot runtime `61c09af` passes both short source runs and the 180-second
repeated AAC pilot: backend/host drift 0.520998/0.849249 ms, peak combined RSS
135440 KiB, exact endpoint, original hashes and complete cleanup. All 18 camera
and six-channel streaming fault trials pass. STOP/CONT retains the actual first
rejection (-298.509959 ms) without advancing accepted sound.

Its first 7200-second attempt overlaps native device fault injection and fails
after 47.417638 seconds, with 14336 prepared frames. The rejected clock records
-20.883398 ms, beyond the unchanged 20 ms limit. Accepted backend/host maxima
are 3.737521/3.422970 ms. Peak combined RSS is 145296 KiB and all children retire.
The overlap is recorded, not asserted as causation. A second 7200-second baseline
without intentional device faults fails after 246.202172 seconds because sound
preparation runs empty. Its accepted backend/host drift stays within
0.894853/0.986766 ms and no rejected clock is recorded. Peak combined RSS is
149904 KiB. Owned children retire and original hashes remain unchanged.
This is a preparation underrun, not a completed sustained playback gate.

Both camera and six-channel native edit/history/save/recovery/reopen/export
workflows pass with exact independent picture/PCM comparison and five edit inputs
each: p95 29.332/31.458 ms. Prepared-picture playback passes for both. Camera's
full native playback regression passes with input/draw p95 25.192/30.572 ms and
129.393 ms cancellation. The six-channel full playback driver remains incomplete:
its first attempt captures a screenshot long enough to miss Pause, and its repeat
waits for an earlier coalesced scrub target while frame 7 has already drawn. Both
raw traces remain. The driver now pauses before screenshot capture and waits for
the final scrub input's acknowledgement; native requalification remains pending.

Default/all-feature workspace tests pass 231/232 plus seven doc tests. Final
all-target/all-feature Clippy passes. A test-only formatting miss after the Copy
cleanup is retained and corrected; see checks/provenance.txt for exact revisions.
The full sustained, physical timing, hardware and adoption gates stay open.
