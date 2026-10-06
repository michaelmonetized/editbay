# Sustained callback clock: current evidence

Issue [#43](https://github.com/michaelmonetized/editbay/issues/43), stacked after
[PR #42](https://github.com/michaelmonetized/editbay/pull/42).
The current frozen app/lab runtime is `b890c16`; full source, lockfile and binary
identities are in [final/](final/). Earlier `ef5bc8d` receipts are in
[candidate/](candidate/). See
[the clock and qualification contract](../../SUSTAINED_PLAYBACK.md).

**The two-hour gate remains open.** These receipts qualify coherent callback
observations, bounded storage, short source playback and explicit failure/retry.
They do not qualify uninterrupted playback while the host experiences sustained
memory stalls. No physical speaker/display drift, audibility or independent-user
acceptance is claimed.

## Final callback implementation

The device route now prefers supported 48 kHz with the default channel count and
sample format; otherwise it preserves the reported default. Candidate `0d2c9ec`
completes a real 180-second repeated AAC run with maximum backend/host drift
0.825966/1.072761 ms, exact endpoint and cleanup; see [rate/](rate/).
The callback now checks continuity before consuming source sound and stops on a
backend timing gap over 20 ms. The original callback failure survives decoder
cancellation and stream teardown.

Frozen `b890c16` passes **220 default tests + 7 doc tests**, **221 all-feature
tests + 7 doc tests**, fmt and all-target/all-feature Clippy with warnings denied.
A real 300 ms STOP/CONT of the owned device worker is detected as backend timing
loss, with both workers reaped in 316.575 ms. The earlier driver candidate stops
but reports generic cancellation instead of the original fault; that failed
classification and the earlier Clippy failure are retained in `earlier/continuity-driver/`.

Both actual native workflows pass ingest, source selection, Play/Pause, exact
sample resume, seeks/end, failure/retry, edit cancellation, save/recovery/reopen
and source preservation. Independently decoded saved PCM agrees exactly.
Camera/six-channel input p95 is 21.259/21.195 ms over 40 inputs each; cached GPU
draw p95 is 19.292/20.076 ms over 38 draws each. Active cancellation is
122.534/122.726 ms; preparation cancellation is 122.617/127.491 ms, playing-worker
death is 89.783/39.225 ms and stopped-picture underflow is 601.054/589.827 ms.
Full traces and the inspected 1440×900 camera / 800×600 six-channel captures are
in [final/regressions/](final/regressions/). At 800×600, expanded edit controls push
the viewer below the visible area; that compact-layout issue remains open.

The actual 7200-second run on `b890c16` started at **09:19:34 UTC on 2026-10-06**
and **fails after 1100.104 seconds** with visible backend timing discontinuity.
Its exact invocation, completed receipt and full periodic observations are in
[final/sustained/](final/sustained/). Maximum observed backend/host drift is
19.030482/25.783440 ms; the callback rejects the next discontinuous observation
before consuming more sound, so the 100 ms trace does not include that rejected
callback as an accepted clock receipt. Source preparation still contains 13824
frames at failure. Sampled combined RSS peaks at 148192 KiB, all owned children
reap, and source/project hashes remain unchanged. Host memory-stall counters rise
near failure while local builds run; that correlation does not establish the
precise cause. Both `qualified` and `two_hour_run_complete` are false. This remains
a failed sustained qualification, not a two-hour result or a source-underflow pass.

## Earlier 2048-frame candidate

| Observation | Camera | Six-channel AAC | Quiet repeated AAC |
| --- | ---: | ---: | ---: |
| Requested duration | 6 s | 6 s | 60 s |
| Elapsed playback/retirement observation | 6.697 s | 6.697 s | 60.875 s |
| Maximum absolute backend drift | 0.029939 ms | 0.026150 ms | 8.440082 ms |
| Maximum absolute host drift | 0.056672 ms | 0.045328 ms | 8.734996 ms |
| Sampled combined RSS peak | 144736 KiB | 146016 KiB | 131552 KiB |
| Exact endpoint, unchanged source/project, child reaping | pass | pass | pass |

The same predeclared limits remain: 20 ms maximum steady clock error, 16384
prepared frames, 4 GiB sampled combined RSS, exact sample end and zero retained
owned children. Backend latency at completion is approximately 211 ms, separate
from physical output timing. The six-channel duration rounds to an exact frame;
its sample endpoint is retained rather than truncated to an integer second.
The minute-long fixture includes the source-loop boundary, authored through the
normal linked command path and checked after undo/redo/save/reopen.

Both actual native prepared-playback regressions pass exact sample resume,
failure/retry and cleanup. Camera/six-channel preparation cancellation measures
151.007/122.422 ms, playing-worker death 88.281/55.662 ms and stopped-worker
underflow 524.968/493.482 ms. Actual sound/device KILL, STOP, cancellation and
preparation-cancellation cases pass, including reaping adopted codec descendants;
maximum retirement across those direct cases is 615.066 ms. Full trace and source
preservation receipts are in [regressions/](regressions/).

Locked local validation passes **218 default tests + 7 doc tests**, **219
all-feature tests + 7 doc tests**, formatting and workspace/all-target/all-feature
Clippy with warnings denied. Complete compressed logs and explicit exits are in
[checks/](checks/). Native builds/tests remain local.

## Preserved failures and limits

- `0ba0344` stops all three short trials after the first callback because it
  assumes the backend timestamp origin cannot change at startup. The ALSA CPAL
  implementation derives time from its trigger timestamp; the measured route
  resets once when output starts. `58efb17` numbers that startup epoch explicitly,
  allows it only within one host second, and rejects subsequent resets.
- `58efb17`, with a 512-frame device request, passes six-second source trials but
  fails its first 7200-second attempt after 7.066 seconds while builds run.
  Maximum backend error is 32.070 ms and the final error is invalid buffer/clock.
  No two-hour completion or gate pass is inferred from its process exit zero.
- `ef5bc8d` requests 2048 device frames while retaining the same 16384-frame
  preparation capacity and drift gates. Its concurrent-build/native-fault stress
  attempt fails after 8.213 seconds from **source preparation underflow**. Backend
  and host clock errors remain 0.079042/0.113614 ms, but this is still a failed
  sustained run. Host memory-stall counters rise sharply near retirement; that
  correlation alone is not proof of the precise scheduling cause. Workers reap.

Those original receipts and periodic traces remain in [earlier/](earlier/).
A separate baseline starts on 2026-10-06 at 08:56:00 UTC, after the local builds
and fault trials finish. It uses the unchanged frozen `ef5bc8d` binaries and
also fails: after 174.323 seconds, maximum backend/host errors reach
116.927/117.822 ms, followed by source underflow. Host-memory stalls increase
sharply, sampled process-tree RSS peaks at 148032 KiB, and all children reap.
The exact receipt and periodic observations are in [earlier/baseline/](earlier/baseline/).
**There is no two-hour pass.** A new candidate prefers supported 48 kHz output
to investigate the accumulated timing error of the default 44.1 kHz route;
its qualification is not inferred from the earlier short results.

Periodic 100 ms samples omit unobserved peaks/callbacks. RSS includes the lab and
its process tree, can double-count shared pages, and excludes external sound
server memory, unmapped cache and unreported device/driver allocations. Steady
clock summaries exclude the first backend second; all startup observations are
retained, and different timestamp epochs are never treated as one clock.
