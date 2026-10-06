# Exact sound coefficient reuse

Issue #51 / PR #52 follows #49 / PR #50. Linux ARM64 local evidence, 2026-10-06.
Source commits, frozen binary hashes, lockfile hashes and host/compiler identities
are retained with each candidate. Large MOV outputs, executable files and private
source media remain under the project artifacts directory.

## Frozen mixer: `0c840ce`

The `compiled` candidate keeps the exact coefficient cache and compiles the
sample loop within the optimized audio crate. Budgets remain 256 kernels /
512 KiB, original PCM/work limits, preparation p95 <=5 ms, warm unity <=20 ms,
sinc <=40 ms, PCM error <=0.000001 and cancellation <=2 s. Queue capacity remains
16384 frames, with 4096-frame producer blocks and a 20 ms clock continuity bound.

| Actual route | Camera | Six-channel AAC |
| --- | ---: | ---: |
| Sinc p95, 30 blocks | 10.702 ms | 15.396 ms |
| Sinc maximum | 83.740 ms | 131.074 ms |
| All sound-block gates | Pass | Pass |
| Coefficient / metadata bytes | 261072 / 18432 | 261072 / 18432 |
| Retained kernels / hits / misses | 147 / 245613 / 147 | 147 / 245613 / 147 |
| Kernel allocations after clear | 0 | 0 |

`ranges-*.json` independently decodes four whole masters at 48/44.1 kHz against
the pre-kernel `fe76821` masters and twelve exact range slices. All picture/PCM
hashes, counts, channel metadata and zero-based output timestamps agree.
`faults` contains 18 actual camera/six-channel full/cancel/codec-death/underrun/
device-death/device-stall/stalled-cancel/preparation-cancel/resume trials: all
qualify, preserve sources and reap children; maximum retirement is 610.598 ms.

`native` and `native-repeat` preserve every attempt. Four initial camera prepared,
camera edit, camera playback and six-channel prepared drivers time out awaiting
callback/GPU acknowledgment during memory pressure. They are incomplete results,
not passes; concurrent all-feature compilation is not established as their cause.
The four separately named repeats pass after that compilation finishes. Initial
six-channel edit and full playback trials also pass. Native edit input p95 is
37.028 / 35.859 ms (five inputs each); saved/recovered graphs and independent
exports agree. Full playback has 40 inputs / 38 completed cached GPU draws per
fixture: input p95 22.189 / 20.773 ms, draw p95 33.410 / 19.670 ms and active
cancellation 119.414 / 124.036 ms. Compact six-channel timeline imagery was inspected.

`checks` passes 239 default / 240 all-feature tests plus 7 doc tests, formatting
and all-target/all-feature Clippy with warnings denied. The audio tests include
bitwise cached/uncached/evicted PCM, reverse/gain/freeze, 44.1/24/96 kHz and the
unchanged independent passband/alias checks.

## Sustained failure and earlier candidates

`compiled/pilot` passes short camera/six-channel runs but its 180-second quiet AAC
pilot fails after **97.648457 seconds**, with no intentional concurrent device
faults. Workspace compilation and export verification overlap this run. The queue
is empty; accepted backend/host drift maxima are **0.734583 / 0.414251 ms**.
Its last completed preparation took 7.868 ms; the slowest completed block took
187.135 ms, of which rendering accounts for 184.984 ms. This completed-block
observation cannot identify the unfinished work. Source identity and child
cleanup pass. The requested 180 seconds are not completed.

`initial` retains uncached baseline sinc p95 81.513 / 169.051 ms and the first
cache candidate's 28.007 / 112.906 ms. `slice` retains 19.311 / 48.034 ms;
`channel` retains 30.884 / 49.393 ms. Every six-channel intermediate candidate
fails the unchanged 40 ms gate. They are not substituted for the passing mixer.

Active-stage telemetry is being qualified after the pilot failure. A two-hour
success has not been obtained. Physical audibility, DAC/speaker drift, other
hardware, full R2/R8 and all remaining roadmap/release/adoption gates remain open.
