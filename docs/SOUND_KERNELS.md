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
misses, evictions and calculated coefficients. Existing PCM pins, cancellation,
source ownership, output charges and interpolation work limits remain.

Streaming preparation records completed and failed block attempts: plan time,
render time, check/routing/publication time, total time, the stages of the slowest
block and the longest interval between block starts. A fixed atomic record is
copied coherently off the callback. Parent validation rejects regressing or
impossible observations. The callback gains no allocations, locks, decode or IO;
its 16384-frame queue and 20 ms clock continuity gate are unchanged.

Bit-for-bit cached/uncached/evicted tests cover 44.1/24/96 kHz, reverse, gain and
freeze. Independent real-media and device qualification follows on frozen builds.
The earlier 246-second preparation underrun is not attributed to coefficient
construction without actual measurements. Two-hour, physical output and full
R2/R8 qualification remain open.
