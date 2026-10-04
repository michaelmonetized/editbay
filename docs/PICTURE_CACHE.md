# Source-owned decoded pictures

`editbay-media::PictureCache` serves exact indexed media requests from an immutable
`EvaluationSnapshot`. Create and use it on its owning worker. It retains native
decoder/file descriptors and immutable `Arc<DecodedPicture>` outputs. It performs
blocking codec/file work and must never run on the UI thread. Native application
IPC and codec process isolation for this cache still need integration; the existing
isolated import worker remains intact.

Each request must match the captured source/stream, asset byte hash/size and cached
interpretation hash. Its picture ordinal must agree with exact source position and
direction. The provider requires an actual VFR presentation index, including the
indexed timestamps produced by native ingest for constant-rate footage. A rate-only
CFR document must be indexed before this provider can decode it. Audio/nested
requests go to their own consumers, not this picture cache.

Assets must have absolute paths resolved within the caller's granted source scope.
First binding owns and fully hashes a read-only descriptor. Every request, including
cache hits, checks cancellation and pathname/inode/size/mtime/ctime ownership.
Misses recheck ownership after native decode. `verify_sources` fully hashes each
source used in the current generation before publication. Receivers still check
document/revision/session/generation ownership; a cache hit grants no permission
to publish into another document or a newer edit.

Sequential picture requests retain the decoder cursor and read the next native
frame. Random/reverse/after-EOF requests seek and require the exact indexed tick;
neighbouring frames are never substituted. Decoder handles and output entries use
least-recently-requested eviction. Source handles remain retained up to their
declared per-job cap so final complete-byte checks retain their ownership.

## Memory and lifecycle

Defaults: 256 MiB cached RGBA8 payload, 384 MiB live output payload, 64 MiB maximum
picture, 128 entries, 16 source descriptors and four decoder handles. Limits are
validated before use. Output allocation is charged before decoding; the charge
stays with the picture until its final `Arc` drops. Evicting a still-held picture
does not erase its live-memory charge. If consumers pin the live budget, decoding
fails explicitly until they release outputs. A picture larger than the cache byte
limit can be served uncached when it fits the live/picture limits.

These counters cover Rust decoded RGBA payloads, including the requested decode
allocation. They do not count native codec/reference buffers, allocator overhead,
GPU images, OS page cache or other workers. Native library bounds, process resource
limits and the full baseline memory gate remain separate requirements. Read-only
pixel access prevents a consumer from modifying shared cache content.

`clear` releases cache-owned pixels/decoders/files and advances generation. External
pixel allocations stay valid and charged, but their old receipts are obsolete.
`rebind(snapshot, fresh_cancel)` cancels the previous job token, resets decoders and
invalidates its receipts. Matching raw content remains reusable. Changed/removed
or stale file bindings are dropped and must pass new source preflight. This lets a
metadata edit reuse pixels while rejecting its previous publication ownership.
Cancellation is permanent for a token; a retry requires a fresh token.

## Qualification

```sh
mkdir -p artifacts/tmp
export TMPDIR="$PWD/artifacts/tmp"
cargo run --locked -p editbay-lab -- picture-cache /path/to/camera.mp4
```

The Rust lab compares 25 retained seek targets against a complete independent
sequential native decode, checks 1,000 cache hits share those verified immutable
allocations, measures 120 sequential requests and joins an actively canceled native
seek. Source checksums are verified before and after the run. Pixel data stays local.

The cache-hit budget is the unchanged 250 ms seek target, measured only for
already-resident raw-picture outputs. Miss/first-binding and sequential timings are
reported separately. Empty output-cache requests retain OS page-cache warmth;
they are not physical cold-disk proof. Cache hits exclude graph rendering, GPU
upload and native presentation. The [evidence](evidence/r2-picture-cache/README.md)
keeps these limits explicit. Full R2 seek, preview/export, hardware, color and
two-hour picture/sound playback gates remain open.

The cache stores the native adapter's current RGBA8 output. It does not apply
working/display/output transforms, enable float/HDR decode, or supply an audio
block. Native source-tag conversion retains its documented ingest limits;
interactive source matrix/range overrides still need an adapter/rendering route.

The cache now retains actual decoded color, alpha/interpretation requirement and
native rotation alongside raw pixels. [The shared SDR GPU picture worker](GPU_PICTURES.md)
uses these to check captured interpretations before input conversion, including
GPU cache hits. Native cache IPC/process isolation and presentation remain open.
