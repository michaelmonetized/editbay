# Exact sound coefficient reuse

Issue #51 follows range delivery (#49 / PR #50). The sound renderer now retains
unchanged Blackman-windowed sinc coefficients for identical phase, cutoff and
relative tap boundaries. It retains original tap order and normalization sums;
absolute source origin and gain do not alter the coefficient identity. Integer
unity-rate samples still use direct copies. No phase or coefficient quantization
is introduced.

The default cache allows 256 entries and 512 KiB including reserved entry metadata
and coefficient vector capacities. Limits are explicit in `SoundRenderBudget`;
zero or insufficient capacity uses the uncached arithmetic. Maximum accepted
configuration is 4096 entries / 16 MiB. Oldest-use eviction is deterministic,
clear releases all cache allocations, and stats report retained bytes, hits,
misses, evictions and calculated retained coefficients. Existing PCM pins, cancellation,
source ownership, output charges and interpolation work limits remain.

The numerical source loop is compiled inside `editbay-audio`, separately from the
generic PCM provider. This preserves the audio crate's optimization when a caller
uses a different build profile. Rust otherwise instantiates generic code under
the caller's settings; see [Cargo's profile rules](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides-and-generics).
The provider retains its original ownership and conditional thread-safety. PCM
lookup is outside the tap loop; tap, channel and source summation order is unchanged.

Streaming preparation records completed and failed block attempts: plan time,
render time, check/routing/publication time, total time, the stages of the slowest
block and the longest interval between block starts. A fixed atomic record is
copied coherently off the callback. Active-stage clocks expose unfinished planning,
rendering and publication work even when the producer cannot complete a block.
Parent validation rejects regressing or
impossible observations. The callback gains no allocations, locks, decode or IO;
its 16384-frame queue and 20 ms clock continuity gate are unchanged.

Bit-for-bit cached/uncached/evicted tests cover 44.1/24/96 kHz, reverse, gain and
freeze. Frozen `0c840ce` passes every sound-block gate on camera and six-channel
AAC: 30 sinc blocks have p95 10.702 / 15.396 ms against the unchanged 40 ms limit.
Recorded maxima are 83.740 / 131.074 ms; p95 does not hide these costs. Each run
retains 147 kernels, 261072 coefficient bytes and 18432 metadata bytes, then
releases all kernel storage. Earlier six-channel candidates fail at 112.906,
48.034 and 49.393 ms p95 and remain recorded. Four whole masters and twelve ranges
preserve pre-change pixels/PCM exactly. Eighteen real sound fault trials pass,
as do native prepared playback, edit/history/save/recovery/reopen/export and full
playback on both fixtures. Four earlier native acknowledgment timeouts remain
recorded separately. The three-minute pilot fails after 97.648 seconds with an
empty queue and less than 0.735 ms measured backend drift; the completed-block
record does not identify its unfinished work. The active-stage extension follows
that failure. See [raw evidence](evidence/r2-sound-kernels/README.md).
Frozen `71f07e4` passes the 18 device fault trials and a fresh 180-second pilot:
backend drift <=1.156 ms, slowest preparation 38.346 ms and peak combined RSS
135920 KiB. Its final locked workspace/all-feature checks, fmt and Clippy pass.
The two-hour baseline fails after 39.359 seconds on clock continuity with a full
16384-frame prepared queue and first rejected drift -29.621 ms. Its raw result is
retained; no sustained success is inferred from the pilot.
The earlier 246-second preparation underrun is not attributed to coefficient
construction without actual measurements. Two-hour, physical output and full
R2/R8 qualification remain open.
