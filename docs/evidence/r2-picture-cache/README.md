# Source-owned picture cache evidence

Linux ARM64 / Apple M1 Pro / Omarchy, Rust/Cargo 1.98.0, 2026-10-04. This qualifies
the raw CPU decoded-picture slice of EB-022/023/025/026.
[Issue #11](https://github.com/michaelmonetized/editbay/issues/11) tracks it.
Native IPC/process integration, GPU graph/output caches, surface presentation,
audio intervals and full R2 release gates remain open.

## Source and artifact ownership

The frozen unoptimized ARM64 Rust lab SHA-256 is
`074740c628dfe4c7879e65b41dd20ad72d63f22cbb70210f58b1fb7e169b6c3b`.
Lockfile `ced164e8dba53bb55bcb933c764a2a2b2a1d8d0865765c06e099148ba82ff064`.
`source-hashes.txt`, `artifacts.txt` and `elf-header.txt` identify compiled inputs
and actual binary architecture; the containing commit identifies source.
Private binaries/raw receipts remain under `artifacts/r2-picture-cache-final/`.
Build and temporary fixtures use `target/native-build/` and `artifacts/tmp/`, both
ignored by Git. No project outputs were written to the root temporary directories.

The camera retains 5,235,017 bytes and source SHA-256
`498ec6c42b6b440886eb761dc4775c994f7578021890ceae3f370ffc6fb3e021`;
the 1080x1920 portrait retains 777,868 bytes and
`872e2e7f3cb19ff13653bea9784c48d78969ceb4b8f7da18d1ea5c7cbaa9a48f`.
These are the actual sources from the previous ingest qualification. The Rust lab
owns read-only descriptors, imports the selected actual video index, and verifies
all bytes before/after qualification. Source pixels stay private; receipts contain
only their checksums, ticks, counts and geometry.

## Software checks

The default full workspace passes **101 tests plus five SDK documentation examples**;
all features pass **102 plus five**, with pinned native libtorch selected. Full
all-feature Clippy with `-D warnings` and formatting pass. `checks.json` records
commands and zero failed/ignored tests. Copied logs/ELF output have trailing spaces
and final empty lines trimmed for Git; original logs remain in `artifacts/`.

Five new native regressions exercise exact VFR sequential/reverse/after-EOF pixels,
multi-stream decoder eviction, immutable shared hits, pinned-payload exhaustion,
eviction/cleanup, canceled hits, changed source pathname ownership, forged source
interpretation/ordinal rejection, oversize uncached output, native geometry guards
and metadata-revision reuse with rejection of obsolete receipts. Existing source,
GPU, audio-clock, CLI/MCP, command, save and recovery tests remain passing.

## Actual raw-picture measurements

Each source has 25 seek targets compared byte-checksum-for-checksum against complete
independent sequential native decode. Every one of 1,000 subsequent hits must share
the previously verified immutable `Arc`. No pixel copy or hash is included in hit
timing. Source metadata ownership/cancellation checks remain included. The next
120 actual frames use retained sequential decode; these do not prove long playback.

| Case | Empty output-cache miss p50 / p95 / max | Shared hit p50 / p95 / max | Sequential p50 / p95 / max |
| --- | ---: | ---: | ---: |
| Camera, 314 actual pictures | 130.739 / 295.999 / 320.471 ms | 0.003333 / 0.005000 / 0.016709 ms | 1.697 / 5.637 / 185.665 ms |
| Portrait, 528 actual pictures | 273.814 / 941.064 / 973.115 ms | 0.003333 / 0.003417 / 0.026083 ms | 2.032 / 3.727 / 31.228 ms |

The first request includes full source binding/checksum/native opening. Empty
output caches retain OS page-cache warmth and are not physical cold-disk proof.
Both hit cases pass the unchanged 250 ms target for this raw CPU kernel. Both
uncached miss p95 values exceed it. The full native seek budget remains open;
graph evaluation/GPU upload/display latency is not included.

Defaults enforce 256 MiB cached payload, 384 MiB live output payload, 64 MiB per
picture, 128 entries, 16 source handles and four decoder handles. Warm target
payloads are 92,160,000 camera bytes and 207,360,000 portrait bytes. Sequential
LRU eviction leaves 265,420,800 bytes in 72/32 entries, below the cache limit;
48/88 eviction events are recorded. Cleanup reports zero cache/live bytes,
entries, source descriptors and decoder handles. External pins remain charged
until their final owner drops; their exhaustion/cleanup behavior has separate
native regression proof. These counters cover output payload, not native codec
buffers/allocator/GPU/OS memory. Actual process HWM is 325,856/347,536 KiB.

After initial source binding and picture decode, a native cache miss seek is
issued and canceled after 10 ms. Both jobs return cancellation and join their
decoder worker. After cancellation, completion is 8.689 ms camera and 3.381 ms
portrait, within the unchanged 2 s budget. Issued-seek and decoded-output counters
distinguish these from preflight cancellation or a cache hit. The originals remain
byte-identical after each run.

## Remaining product proof

This is a functioning raw CPU provider and Rust qualification tool. The native
application does not yet expose composition preview, source-cache IPC, transport
or export through it. Cached outputs are RGBA8 with the documented native ingest
conversion limits. Float/HDR decoding, input matrix/range overrides, working and
display/output transforms, GPU texture handoff and bounded working-image caches
remain open. Audio keys still need interval/routing/resampling implementation.
No native displayed/exported composition, two-hour picture/sound drift, physical
audibility, additional hardware or enterprise/GTM completion is claimed.
