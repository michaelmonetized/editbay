# Sustained callback clock: current evidence

Issue [#43](https://github.com/michaelmonetized/editbay/issues/43), stacked after
[PR #42](https://github.com/michaelmonetized/editbay/pull/42).
The current frozen app/lab runtime is `ef5bc8d`; full source, lockfile and binary
identities are in [candidate/](candidate/). See
[the clock and qualification contract](../../SUSTAINED_PLAYBACK.md).

**The two-hour gate remains open.** These receipts qualify coherent callback
observations, bounded storage, short source playback and explicit failure/retry.
They do not qualify uninterrupted playback while the host experiences sustained
memory stalls. No physical speaker/display drift, audibility or independent-user
acceptance is claimed.

## Current candidate

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
