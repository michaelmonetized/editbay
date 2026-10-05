# Shared resident GPU pictures

`editbay-render::GraphRenderer` consumes the retained `EvaluationSnapshot` and
source-owned `PictureCache`. Its functioning SDR picture subset covers media and
nested composition sources, solids, animated affine transforms, unmasked over,
scalar animation and opacity. Audio is evaluated separately. Geometry/masks,
mask assets, HDR/log input, PQ/HLG boundaries and source orientation return errors.
This is a scoped EB-024/026 engine layer; native preview and delivery integration
remain open. No unsupported native controls are enabled.

## Evaluation and color

Create/use/drop the renderer on its owning worker. Device/pipeline creation,
source hashing, native codec calls, graph evaluation and GPU waits stay off the
UI thread. `render(composition, exact_position, before)` prepares the same exact
fractional/reverse temporal plan used by inspection. Nested scenes retain exact
child positions and boundary sides. Timed inactive images become transparent;
inactive scalar values become zero. Root picture dependencies determine rendering;
reachable audio dependencies perform no picture decode or dispatch.

Working textures are premultiplied **linear** RGBA16Float or RGBA32Float. Solids
supply straight linear RGB in the document's working primaries and normalized
alpha. This subset accepts finite solid channels within +/-4096. Opacity multiplies
premultiplied RGB and alpha. Over uses foreground plus background times one minus
foreground alpha. No exposure/background is silently added by the renderer.

Sources use the native adapter's **RGBA8** payload. Captured color/alpha must agree
with actual decoded metadata; unsupported matrix/range/transfer, unknown source
primaries/alpha and interpretation overrides fail. Decoded rotation is checked
alongside document metadata, so deleting its metadata cannot hide a rotated file.
Native color fallback now uses the container tags for both indexing and pixel
conversion when frame tags are unspecified. Full source hashes and file ownership
checks remain intact. High-bit-depth originals retain their bytes but this path
quantizes decoded input to RGBA8; it does not prove float/HDR decode.

Supported input transfers are linear, sRGB and BT.709; source primaries are BT.709,
BT.2020 or Display P3. Native matrices supported here are RGB, BT.709, FCC,
BT.470BG/SMPTE170M, SMPTE240M and BT.2020 non-constant luminance with declared range.
Input transfer is removed per texel before bilinear interpolation. Encoded
premultiplied source RGB is divided by alpha before transfer removal, then linear
RGB is premultiplied. The actual media qualification uses opaque tagged footage;
alpha math is separately exercised through typed solids/compositing.

Source and nested images fit inside the composition, centered with transparent
outside pixels. Source pixel aspect participates in fitting. Coordinates use pixel
centers and an upper-left origin. Affine scale/rotation act about the composition
center; translation is in composition pixels; positive rotation is clockwise in
this coordinate system. Zero scale produces transparency. Manual texture loads
and bilinear weights work with unfilterable FP32 textures.

`convert(receipt, Display|Output)` applies the corresponding captured gamut and
transfer after working evaluation. These boundaries do not alter working-cache
content. Linear/sRGB/BT.709 destinations are supported. D65 RGB/XYZ matrices
convert among BT.709, BT.2020 and Display P3; negative/extended float channels stay
unclamped. There is no tone mapper, display calibration, ICC/OCIO transform or
absolute-luminance HDR contract. Such workflows remain their release gates.

Primaries/transfer references: [ITU BT.709](https://www.itu.int/rec/R-REC-BT.709),
[ITU BT.2020](https://www.itu.int/rec/R-REC-BT.2020). Native GPU completion follows
[wgpu's queue ownership contract](https://docs.rs/wgpu/30.0.1/wgpu/struct.Queue.html).

## Ownership and limits

Defaults allow 512 MiB cached textures, 768 MiB live texture payload, 128 MiB per
texture, 256 cache entries, 4,096 evaluated picture nodes including nested calls,
32 nested scenes and 16 outstanding submissions. These are additional to the raw
picture provider's declared budget. Device-supported dimensions/formats and
caller limits are checked. Cache entries use least-recently-used eviction.

The live charge follows the immutable `Arc<ResidentImage>`, including temporary
uploads, consumer-held outputs and submitted input/output pins. GPU-completion
callbacks release in-flight pins; eviction cannot erase their charges. Exhausted
pins cause an error until consumers release outputs. Read-only image/receipt APIs
expose no mutable texture or clonable raw GPU handles that bypass these charges.
Dead graph inputs are released after their final use. This counts texture payload,
not driver allocations, native codec buffers, pipeline/uniform overhead, upload
staging, readback buffers, allocator overhead or OS caches. Explicit readback has
one bounded padded buffer plus one float output. Whole-process memory evidence
remains separate from these counters and from the 4 GiB product baseline gate.

Working keys include renderer semantics, prepared content and actual input keys;
source keys include captured color/alpha/pixel aspect. Display/output keys include
their destination transform. Source requests still pass the raw provider on GPU
hits to verify file ownership and actual interpretation; evicted raw outputs can
require decoding even when a GPU texture is reusable. Nested keys inherit the
compiler's conservative semantic invalidation documented in `TEMPORAL_EVALUATION.md`.

Receipts capture private worker identity, document/revision and generation.
`validate_result` rejects foreign/stale/cancelled receipts. `rebind` uses a fresh
cancellation token, invalidates old receipts and reuses matching content. `clear`
waits for GPU work, releases owned caches/handles and advances generation;
external pins remain charged. `verify_sources` fully rehashes used sources before
publication. A receiving UI/exporter still checks its session/job ownership.

Cancellation reaches native source work and is checked between nodes and during
GPU waits. Waits poll in 20 ms intervals with a 2 s total limit. Already-submitted
GPU commands are not preempted; their payload stays charged until completion.
Cleanup and worker drop also drain submissions. Lost/stalled devices still need
native supervision/retry; this library does not claim process crash containment.

## Qualification and next integration

```sh
mkdir -p artifacts/tmp
export TMPDIR="$PWD/artifacts/tmp"
cargo run --locked -p editbay-lab -- render-graph /path/to/camera.mp4
```

The Rust lab compares three actual pictures at both precisions with independent
CPU transfer/alpha-over expectations; verifies 1,000 shared hits per precision;
measures 120 ordinal-mapped native pictures through GPU completion without normal
path readback; checks explicit encoded output and active native seek cancellation;
then verifies original checksums and zero owned payload/handle cleanup.
The first source-binding picture is reported separately from the 119 steady
requests. The predeclared picture-kernel budget is p95 <=33.3 ms, with FP16 error
<=0.002 and FP32 <=0.00002. Source-ordinal mapping is explicitly a qualification
fixture, not natural VFR wall-clock playback. See [evidence](evidence/r2-gpu/README.md).

The same graph now accepts the [isolated retained codec provider](WORKER_PICTURES.md),
with private receipt validation before upload and qualified real source/output
agreement. This worker still uses its own headless device. Shared native UI-device
presentation, GPU display-surface quantization, transport/frame scheduling,
audio blocks/clock, export encoders, masks/HDR and complete R2 hardware/playback
qualification remain next work. `readback` is an explicit float inspection/export
boundary, not a preview implementation or a delivery-file exporter.
