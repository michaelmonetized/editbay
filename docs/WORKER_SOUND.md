# Isolated native sound

Issue #21 builds on sound blocks in PR #20. A packaged app, CLI or lab executable
hosts `--pcm-worker` before its normal initialization. The same `SoundRenderer`
accepts native or isolated PCM through `PcmProvider`; sample planning, gain,
nesting, exact unity copies and sinc interpolation share one implementation.

Each child owns one validated immutable document, retained original-channel
native decoders and source descriptors. A private Unix socket transports bounded
metadata and at most one sealed anonymous file descriptor per response. PCM is
written directly into the charged mapping, sealed against changes, then mapped
read-only in the parent. Samples keep their original rate, channel order, float
headroom and absolute sample boundaries. Declared presentation edges are zero;
missing in-range samples and nonfinite output fail visibly.

Session, job, worker, document version, generation, serial, source, stream,
interval, rate, channel identities and exact descriptor size are checked. Seals
prevent truncation or mutation while a consumer holds samples. Result payloads
and ownership stay private. Cache hits still contact the child to recheck source
identity; durable publication verifies complete source checksums. Both routes
must own the exact same evaluation snapshot as the sound compiler.

The picture and sound supervisors share process launch, bounded response reads,
stderr retention, timeout and shutdown. Socket shutdown cancels native work;
a child that has not exited after 100 ms is killed and reaped. Errors invalidate
PCM receipts and release cache ownership. Retry requires a fresh cancellation
token and rebind. Explicit cleanup permits lazy reuse; held output mappings
remain charged until their final consumer releases them.

Default child PCM limits: 32 MiB cache, 64 MiB live output, 64 entries, eight
sources, two decoders, 131072 frames per interval and 262144 decoded frames per
request. Each native decoder's adapter scratch is bounded at 16 MiB. The parent
independently limits mapped output to 64 MiB and 128 handles, including consumer
pins after eviction, crash, cleanup and rebind. Child cache entries cannot exceed
128. Kernel process limits remain 3 GiB virtual memory, 900 CPU seconds and 512
descriptors. Codec-library internal memory is
measured separately in process high-water memory.

Predeclared qualification: already-decoded 4096-frame PCM handoff p95 <=10 ms;
warmed shared unity sound blocks p95 <=20 ms; cancellation/join/reap <=2 s;
combined sound-only parent/child high-water memory <=512 MiB. Existing sample
comparison tolerance remains 0.000001. These worker measurements do not establish
a callback deadline or physical audio latency. The graph qualifier also retains
its existing 5 ms preparation, 40 ms sinc and 250 ms first-render gates.

```sh
export TMPDIR="$PWD/artifacts/tmp"
editbay-lab pcm-worker SOURCE PACKAGED_APP_OR_CLI
editbay-lab sound-blocks-worker SOURCE PACKAGED_APP_OR_CLI
```

Both commands return individual gates and an aggregate `qualified`. Check that
field as well as process exit status. The source stays read-only. Raw PCM
qualification checks independent sequential FFmpeg samples, 1000 shared handoffs,
worker death/retry, active and SIGSTOP cancellation, pins and cleanup. Graph
qualification uses the same nested two-contribution fixture as native sound.
Native tests also compare reverse, fractional rate, gain and mix across routes.

Natural source-sequence sound tails, native streaming callback scheduling, device
routing, sustained playback and drift, shared delivery and remaining R2 gates
follow this dependency. No enabled playback control is implied by a worker route.
