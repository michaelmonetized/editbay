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

The next runtime pairs ALSA timestamp and delay from one status snapshot and
records the first rejected clock separately. Qualification is in progress; this
page does not mark the sustained gate complete.
