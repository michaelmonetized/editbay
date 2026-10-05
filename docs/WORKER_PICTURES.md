# Isolated retained pictures

`editbay-media::picture_worker::PictureWorker` runs the retained picture provider
inside the packaged app or CLI's Rust `--picture-worker` endpoint. It uses the
same indexed source selection and cache as `PictureCache`, and implements the
same `PictureProvider` consumed by `GraphRenderer::with_provider`. Exact temporal
mapping, SDR color, GPU caches and output conversion have one implementation.
This completes the process handoff dependency of EB-022/025/026. The
[native viewer](NATIVE_PREVIEW.md) now uses the same provider and graph on the
window device. Sound scheduling and delivery remain subsequent R2 work.

## Source and result ownership

Construction takes the executable, immutable `EvaluationSnapshot`, validated
`WorkerBudget` and cancellation token. It belongs on a worker thread. The child
validates the captured document before binding and only serves media requests
that match its source/stream, asset checksum/size, interpretation and exact index.
Captured assets are absolute paths within the caller's granted source scope.
This library does not grant additional MCP access or expose a network service.

The child retains fully checksummed read-only source descriptors. Native positioned
reads cannot reopen a replacement pathname. Every request, including a raw/GPU
cache hit, checks source identity and cancellation. `verify_sources` rehashes all
used source bytes before final publication. The native adapter decodes delayed
pictures from the preceding keyframe, converts only the exact requested picture,
and writes it directly into an exclusively owned shared output. Independent
sequential decoding remains the pixel reference. Backward and after-EOF requests
retain exact timestamps; neighbouring pictures cannot substitute for a target.

Private messages carry session, job, worker, document/revision, generation and
ordered request serial. Picture replies also return the exact source request and
actual dimensions, tick, color, alpha/interpretation requirement and rotation.
Foreign, obsolete, unexpected and malformed replies fail before upload. Receipt
ownership includes a private nonce renewed on rebind/clear; changing a receipt's
public version/generation cannot revive it. The receiving UI/exporter must still
check its captured document tab/session before publication.

## Shared bytes and budgets

A private Unix stream transfers bounded JSON metadata and at most one
`SCM_RIGHTS` descriptor per message. RGBA pixels never enter JSON/base64. The
child writes a `memfd`, unmaps its writable view, then applies `F_SEAL_WRITE`,
`F_SEAL_GROW`, `F_SEAL_SHRINK` and `F_SEAL_SEAL`. The receiver requires a regular
file of exact checked geometry/length with all four seals before mapping it
read-only. Truncation, mutation and shared writable mappings are refused by the kernel.
Received descriptors are CLOEXEC. Unexpected/extra/truncated ancillary handles
are closed on every error path.

Defaults retain 256 MiB cached RGBA, 384 MiB live mapped outputs, 64 MiB per picture,
128 entries, 16 sources and four decoders in each provider. The parent additionally
limits consumer-held mapped handles to 256. Process configuration caps cache
entries/parent live handles at 256 and live output bytes at 1 GiB. The child soft
limits remain 3 GiB virtual memory, 900 CPU seconds per job and 512 descriptors,
or smaller inherited limits. Native decoder/index limits remain in
[media ingest](MEDIA_INGEST.md). Metadata is length-framed and limited to 8 MiB;
request and response queues each hold one message. Stderr retains an 8 KiB tail.

Live charges follow the immutable `Arc<DecodedPicture>` until its final owner
drops. Eviction, clear, worker death or cache rebinding cannot erase consumer
charges or invalidate their sealed bytes. Matching cache content can share its
existing mapping under fresh receipt ownership. Exhausted bytes or handles fail
until consumers release pins. The child also retains bounded sealed outputs;
decoded output has no second RGBA-vector copy in this route.

Counters describe mapped/retained payloads and handles, excluding native codec
reference buffers, allocator overhead, GPU uploads/textures, kernel bookkeeping
and a bounded in-flight descriptor. Shared backing pages can remain allocated
without appearing in a process's RSS until its mapping is touched; adding parent
and child RSS also double-counts touched shared pages. Payload and process receipts
are complementary evidence, not a universal 4 GiB qualification.

Linux contracts: [sealed memfd ownership](https://man7.org/linux/man-pages/man2/memfd_create.2.html),
[Unix descriptor transfer](https://man7.org/linux/man-pages/man7/unix.7.html),
[CLOEXEC and ancillary truncation](https://man7.org/linux/man-pages/man2/recvmsg.2.html).
Native GPU upload remains an explicit copy. This is process crash containment;
it does not advertise an OS security sandbox or universal zero-copy rendering.

## Cancellation, cleanup and retry

The parent checks cancellation while awaiting actual replies. Closing the private
socket requests cancellation of the child's native IO/decode. The supervisor
allows 100 ms to exit, then kills and reaps a stalled child; reader/stderr threads
join. A parent-thread/process death also terminates its child. Writes have a 250 ms
socket timeout; an operation has a 120 s reply deadline. These are failure bounds,
not performance budgets.

Worker/protocol failure remains visible and requires `rebind(snapshot, fresh_token)`
to retry. Rebinding renews the job/generation and rejects previous receipts while
retaining matching immutable content. `clear` releases owned child/source/cache
resources and renews ownership; subsequent use starts a fresh codec lazily. External
pins remain valid and charged. Source originals are never written or replaced.

## Qualification

```sh
mkdir -p artifacts/tmp
export TMPDIR="$PWD/artifacts/tmp"
cargo run --locked -p editbay-lab -- picture-worker SOURCE [WORKER_BINARY]
cargo run --locked -p editbay-lab -- render-graph-worker SOURCE [WORKER_BINARY]
```

The first command checks real indexed pixels against full independent sequential
decode, reverse EOF, 1,000 already-decoded immutable handoffs, misses/startup,
120 sequential pictures, worker death/rebind, active and SIGSTOP cancellation,
full source checksums and zero owned cleanup. The separate handoff budget is
**p95 <=10 ms** for an already-decoded 1080p RGBA plane. No pixel hashing/copy is
timed in cache hits; native identity checks, transport, envelope/seal validation
and shared mapping lookup are included. Source misses are measured separately.
The second command runs the established typed GPU/color reference through this
provider at both precisions. See [evidence](evidence/r2-picture-worker/README.md).
Full native warm/cold seek, sound/drop/drift, preview/export delivery
parity, other hardware and enterprise/GTM gates remain open.
