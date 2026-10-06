# Native sound presentation time

Issue [#59](https://github.com/michaelmonetized/editbay/issues/59) follows
[PR #58](https://github.com/michaelmonetized/editbay/pull/58). The prior two-hour
ALSA attempt stopped after 39.359 seconds with a full source queue and rejected
backend drift of -29.621 ms. Its cause remains unproven. Later source preparation
passes a 180-second pilot; the two-hour gate remains open.

The packaged Rust sound worker now enables CPAL's native PipeWire host, preferred
when available, with ALSA retained as the platform fallback. Validated device
receipts name the actual host alongside device, format, rate and callback sizes.
No desktop/server configuration is changed.

The native adapter records queued buffer sizes in frames. Presentation time adds
rational graph delay and stream-rate queued/resampler frames to the captured
graph timestamp, using checked integer arithmetic. The callback timestamp is the
current monotonic time in that same clock domain, so callback scheduling delay
does not become extra reported latency. Invalid/overflowing clocks fail visibly.
Before a valid graph clock exists, up to one second of equilibrium may be queued
without consuming source samples. Missing timing after startup fails.

A scoped native buffer guard owns dequeue/return, bounds mapped interleaved
geometry, sets both chunk bytes and frame accounting, and reports queue failure.
The ALSA fallback submits every frame consumed by the application callback;
partial writes advance through the same prepared bytes. Zero progress, invalid
write counts, EAGAIN, EPIPE and suspended writes fail instead of discarding samples
or spinning. Capture behavior is unchanged.

The application callback still only consumes bounded prepared PCM and updates
fixed atomics. Source planning/decoding, filesystem access, serialization and
status locking remain outside it. Queue capacity 16384, preparation blocks 4096,
requested callback 2048 and the 20 ms continuity guard are unchanged. Existing
worker ownership, cancellation, visible errors and cleanup remain in force.

Local backend tests cover partial-write byte order and error paths, native buffer
geometry, distinct stream/graph rates, rational graph rates, negative delay,
rounding, malformed timing and overflow. Production compilation passes; actual
backend, worker fault, native workflow and sustained qualification are in progress.
No physical audibility, speaker latency or later release gate is claimed.

The vendored adapter and licensing provenance are documented in
[CPAL changes](../vendor/cpal/EDITBAY.md). Timing follows PipeWire's
[time contract](https://docs.pipewire.org/structpw__time.html) and
[buffer contract](https://docs.pipewire.org/structpw__buffer.html).
