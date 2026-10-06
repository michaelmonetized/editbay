# Native sound blocks

Issue #18 follows the qualified native picture viewer. The bounded worker
implementation is locally qualified in [sound evidence](evidence/r2-sound/README.md).
Audio-clock playback and delivery remain separate release gates.

The core compiles reachable source, gain, mix and nested paths once from the
validated `EvaluationSnapshot`. Each prepared block owns exact rational output
sample centers, original source sample centers and evaluated gain. Private owner
identity rejects plans from another compiler even when public versions match.

Source samples are centered at n + 1/2. The renderer converts a source center to
PCM coordinate center - 1/2. Reverse 1:1 therefore starts at the final included
sample, not the exclusive end. Inactive ranges/tracks and zero-speed freezes are
declared silence. Failed sources, gaps and premature in-range EOF are errors.

Channel identities/order are preserved. There is no implicit mono/stereo/surround
conversion. Gain and mixing use float headroom and reject nonfinite output.
Integer 1:1 forward/reverse samples are copied exactly. Other positions use a
48-lobe Blackman-windowed sinc; downsampling uses cutoff 0.95 / absolute source
step, normalized over the complete kernel. Presentation-edge padding is zero.
The source step is composed through every nested time-map segment. Steps above
16 source samples per output sample are rejected before media IO.

Default core bounds: 4096 output frames, 64 source paths, 256 path steps,
262144 resolved positions, 1048576 primitive operations. Native PCM bounds:
131072 frames per request, 32 MiB cache, 64 MiB live PCM, 64 entries, 8 retained
sources, 2 decoders, 262144 decoded frames per request. Each decoder has at most
65536 x 64 float samples of adapter scratch (16 MiB); codec-library internal
memory is separate and measured through process high-water memory. Output pins
remain charged after eviction or cleanup. Render bounds: 32 MiB consumer-held
output and 16777216 channel/tap operations per block. Mixing scratch is at most
8192 x 64 doubles (4 MiB), plus bounded source-position records. Requests,
hashing, decoding, interpolation and mixing belong to workers, not callbacks.

Predeclared qualification budgets, before timing: 4096-frame two-contribution
nested gain/mix preparation p95 <= 5 ms; warmed native unity-rate render p95
<= 20 ms; two-channel 48 kHz to 44.1 kHz sinc render p95 <= 40 ms; first native
PCM render <= 250 ms; cancellation <= 2 seconds; software process high-water
memory <= 256 MiB for this bounded sound-only qualification. These are worker
measurements, separate from the eventual device callback deadline.

Correctness gates: lossless original samples/channels unchanged; decoded AAC
absolute difference <= 0.000001 against independent sequential FFmpeg PCM;
analytic 1 kHz passband error and 18 kHz alias at 24 kHz output <= 0.0001 away
from presentation edges. Frozen binary/source hashes and every failed candidate
must accompany the final measurements. None of these proves physical audibility.

The native source-sequence factory still authors picture only. Audio ending
between sequence frames needs an explicit natural-time/silence-tail contract;
compressing sound to the rounded picture tail would introduce drift. Native
streaming callback scheduling, device routes, two-hour drift, full shared delivery
and the R2 hardware matrix remain open. The renderer now shares its evaluation
with [isolated native PCM](WORKER_SOUND.md); that route preserves the sound block
contract and derives cancellation from its underlying provider.
