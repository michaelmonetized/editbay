# Procedural GPU masks

Polygon geometry, feathered masks and masked Over now run in the shared resident
GPU graph used by native preview and master delivery. The saved Rust document and
command path remain the source of truth. This implements procedural evaluation;
asset-mask formats, drawing tools and the R4/R5 artist workflows remain open.

## Pixel rule

Coordinates and feather widths are composition pixels, with the same upper-left
origin and pixel centers as picture transforms. Polygon fill uses even-odd ray
crossings, including self-intersections. Each pixel also measures its distance to
the nearest closed polygon segment; repeated vertices are point segments.

Let `d` be that distance, positive inside and negative outside. Coverage is
`smoothstep(-w/2, w/2, d)`, where `w = max(feather, 1 pixel)`. Thus zero feather
still has a one-pixel antialiasing band. A collinear or collapsed polygon has no
interior; its boundary can retain the outside half of that band. Inversion uses
`1 - coverage`. There is no implicit winding correction or geometry repair.

Geometry, masks and pictures have distinct runtime types. A mask scales all four
premultiplied foreground channels before Over. A transparent background does not
bypass a nontrivial mask. Inactive geometry is empty; an active inverted mask over
empty geometry covers the full picture. An inactive mask is zero regardless of
inversion. Animated feather, nested time and reverse boundary sides come from the
existing exact temporal evaluator.

Mask textures store coverage in all four float channels. They do not apply a
source color transform. Working precision, dimensions, geometry, resolved mask
parameters and dependency identities enter existing semantic cache keys. Corrected
geometry cannot reuse old pixels, and stale/foreign receipts still fail.

## Bounds and ownership

Default graph limits add 1024 vertices per polygon and 67,108,864 pixel-edge tests
per render request. Configurable maxima are 4096 vertices and 268,435,456 tests.
Coordinates must be finite and within +/-1,000,000 pixels. Document validation
continues to enforce at least three vertices and nonnegative finite feather.

Each uncached mask charges width × height × vertex count before its dispatch.
The remaining count is shared across nested evaluations. Cached masks cost no new
pixel-edge work. Earlier bounded dispatches may already have completed when a
later mask exhausts the request limit; the failed request publishes no frame.
Its textures remain subject to the existing cache, consumer-pin and completion
charges. Cancellation, clear and drop retain the same retirement path.

Prepared geometry adds at most node-limit × vertex-limit × 8 bytes of coordinate
payload (32 MiB at default limits), in addition to the captured document and
container overhead. A dispatch uploads at most vertex-limit × 8 bytes, or an
8-byte neutral binding. At most the pending-submission limit plus the currently
prepared dispatch can retain these buffers: 136 KiB of vertex payload by default.
These are separate from texture payload, pipeline/driver/staging allocations and
whole-process memory. Geometry and uniform buffers stay owned by submitted wgpu
commands until completion. The callback contains no mask computation.

Unsupported asset masks fail visibly before graph dispatch. The renderer does
not guess an image/matte format or substitute a fake result.

## Qualification

`cargo test -p editbay-render --test masks --locked` compares actual FP16 and FP32
GPU readback with separate f64 reference math over convex, crossed, repeated,
collinear, collapsed and large-coordinate fixtures. It checks inversion, animation,
inactive boundaries, reverse nested time, shared request limits, cache corrections,
foreign/stale receipts, cancellation and charged-pin cleanup. Declared fixture
errors are <=0.002 for FP16 and <=0.00002 for FP32; these are measured fixture
bounds, not a universal bound for every possible coordinate.

`editbay-lab mask-graph SAVED_SOURCE_PROJECT NEW_DIRECTORY` authors a real animated
mask through commands, checks undo/redo/save/recovery/reopen, compares separate
mask math over shared unmasked source-color pixels, and exports through the real
supervised delivery worker. Installed FFmpeg decodes selected master frames for
an independent byte comparison. Source color evaluation is shared in this lab;
the synthetic mask tests have fully independent input pixels.

`editbay-lab native-masks APP_BINARY MASK_PROJECT NEW_DIRECTORY` exercises actual
wide/compact windows, seeks, visible picture bounds, compact cut creation,
undo/redo/save, exact interval comparison of native cut exports, and reopen.
It requires the sibling verified `Master.mov` created by `mask-graph` and preserves
screenshots and raw diagnostics. See [actual evidence](evidence/r2-procedural-masks/README.md). Native
GPU completion and captured window pixels do not prove physical display timing,
audibility, artist/client approval or independent-user acceptance.
