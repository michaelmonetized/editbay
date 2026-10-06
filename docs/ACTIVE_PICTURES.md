# Reuse proven picture identities

The shared Rust picture renderer retains whether an immutable resident image is
known to contain zero premultiplied RGBA. This proof comes from validated shader
parameters and already-proven input images. Decoded media uploads always begin
with unknown content; no pixel inspection or CPU readback establishes this flag.

After validating a node's operation and parameters, the renderer can reuse:

- The background when an Over foreground is known transparent.
- The foreground when an Over background is known transparent.
- The input when scalar opacity is exactly one.
- A canonical transparent image when a supported operation's inputs prove every
  output channel is zero. This includes alpha-zero solids and already-transparent
  nested, transform and opacity inputs.

Reuse requires matching dimensions, float precision, working gamut and linear
encoding. A known-transparent result with another geometry uses that geometry's
canonical transparent texture. Display/output conversions retain their declared
encoding and interpretation; unsupported HDR still fails before conversion.
Zero opacity alone does not assert unknown input pixels are finite or zero.
Solid range, inverse-transform projection and active unsupported-operation checks
all remain in front of simplification.

## Ownership and work

An alias retains the same immutable `Arc<ResidentImage>` and its existing texture
allocation. The renderer creates no cache entry or extra charge for each alias.
Consumer-held results and submitted GPU work continue owning the allocation;
cache clearing cannot erase those charges. Publication still requires the private
worker/generation and captured document version, even when content is reused.
Source verification, cancellation, actual GPU completion and no-readback native
presentation keep their existing paths.

`GraphStats::simplified_nodes` counts nodes resolved as a reused input or canonical
transparent result. Dispatch, upload, cache-entry/byte, live-allocation and pending
submission counters still measure actual work and storage. The full reachable
graph remains prepared, inspected and charged against the existing node/depth
budgets. This change reduces GPU work; CPU preparation and native decode costs
remain measurable parts of the complete frame path.

Native diagnostics report queue delay, rendering, display conversion, draw
preparation and GPU completion separately. Cumulative temporal-preparation and
source-picture microseconds measure those operations across completed requests;
differences between consecutive observations give interval costs. The actual
native adapter/driver is included. These counters describe work, not a replacement
for request-to-presentation, skipped-frame or sound-clock measurements.

## Qualification

Actual GPU tests exercise both FP16 and FP32. A 128-layer sequential graph renders
four forward/reverse-boundary positions with five dispatches and five cached
textures total. Its independently expected pixels agree, and its four held outputs
retain four distinct allocation charges after cache clearing. A nested transparent
image with different geometry exercises transform, partial/unity opacity,
compositing, output gamut/transfer, stale results and invalid transform/HDR errors.
Existing source interpretation, cache eviction, pin, cancellation and actual
surface-presentation checks remain required.

Issue [#34](https://github.com/michaelmonetized/editbay/issues/34) is published in
[PR #37](https://github.com/michaelmonetized/editbay/pull/37), following
[PR #35](https://github.com/michaelmonetized/editbay/pull/35). The parent's native
128-cut camera workload recorded 98 skipped frames, 3,194 dispatches and 3,158
cache evictions, with the sound clock at record frame 127 while the viewer showed
118. The smaller six-channel fixture recorded zero skips and 8,976 dispatches.
Those are retained baselines. Repeated native trials must report actual input,
seek, frame, memory, dispatch and master results with exact executable provenance.

The existing release frame/seek/memory/cancellation gates remain unchanged.
A reduction in dispatches alone does not establish sustained 1080p/4K playback,
physical audibility, two-hour hardware drift or qualification on another GPU.
