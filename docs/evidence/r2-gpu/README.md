# Typed resident GPU picture evidence

Linux ARM64 / Apple M1 Pro / Omarchy, Rust/Cargo 1.98.0, 2026-10-04. This qualifies
the functioning SDR picture subset of EB-024/026, tracked by
[issue #13](https://github.com/michaelmonetized/editbay/issues/13).
[GPU picture contracts](../../GPU_PICTURES.md) define exact ownership and limits.
Native presentation/codec IPC/audio/playback/delivery and full R2 remain open.

## Frozen inputs and checks

The unoptimized AArch64 lab SHA-256 is
`f5bee3de1d334609c4d38844d70fad88d7cd8c9fc253d9b2799646af72aaf0c1`.
Lockfile `c96ece330591b36c3537e8e72eff6fce15ab7f35e6ae9322d0f36d6ca6dbd6df`.
`artifacts.txt`, `elf-header.txt` and `source-hashes.txt` identify the actual binary
and selected compiled inputs. The containing commit identifies source. Private binary,
receipts and original logs remain in `artifacts/r2-gpu-final/`; builds/temporary
fixtures stay in `target/native-build/` and `artifacts/tmp/`. Both parent directories
are ignored. No raw pictures/binaries are committed.

The selected camera has 314 actual indexed pictures, stream 1, 1280x720, 5,235,017
bytes, SHA `498ec6c42b6b440886eb761dc4775c994f7578021890ceae3f370ffc6fb3e021`.
The portrait has 528 pictures, stream 0, 1080x1920, 777,868 bytes, SHA
`872e2e7f3cb19ff13653bea9784c48d78969ceb4b8f7da18d1ea5c7cbaa9a48f`.
Both are opaque tagged BT.709. Originals are read-only and fully verified before
and after qualification. Actual picture 0 checksums also match the preceding
raw-picture cache receipts. Actual adapter is Apple M1 Pro (G13S C0), Vulkan,
Honeykrisp, Mesa 26.2.3-arch1.1; this is GPU execution, not a software renderer.

The full workspace passes **109 tests plus five SDK documentation examples**;
all features pass **110 plus five** with pinned native libtorch. Zero failed or
ignored. All-feature/all-target Clippy `-D warnings` and formatting pass.
`checks.json` records commands; copied logs/ELF metadata have trailing whitespace
trimmed for Git while originals remain private.

Eight new actual GPU regressions exercise both float precisions, transfer/alpha
over, animated scalar opacity, translation/scale/rotation/mirror/zero/fractional
pixel edges, fractional nested and reverse-step boundaries, node/depth limits,
transparent inactive ranges, gamut/output separation, working/display/revision
invalidation, foreign receipts, source pixels/replacement/interpretation refusal,
pixel aspect and actual hidden rotation, pinned eviction, cancellation and cleanup.
Unsupported masks/oversized solid values return errors. Existing command, save,
recovery, CLI/MCP, native ingest/cache, worker, GPU and audio-clock tests pass.
Native recovery/window qualification is retained from the preceding stack; this
slice changes no native UI flow and does not claim a new window qualification.

## Actual completed-picture measurements

The five-node fixture renders source, solid background, scalar opacity, opacity
image and over, through the production temporal/source/GPU path. Expected pixels
come from independent sequential native decode plus CPU transfer removal and
`linear_source * 0.8 + 0.004`, alpha 1. Three pictures per source/precision, ordinals
0/60/119, compare every output channel. Maximum linear error is **0.000418425**
FP16 against the predeclared 0.002 bound, **0.000000119209** FP32 against 0.00002.
Encoded sRGB output maximum error is 0.000467301 / 0.000000119209, within separately
declared 0.004 / 0.00004 bounds. Equal display/output settings share their actual
converted texture; neither changes the working result.

| Case | Resident shared-hit p95 | First empty-cache picture | Steady completed-picture p50 / p95 / max |
| --- | ---: | ---: | ---: |
| Camera FP16 | 0.229 ms | 194.421 ms | 3.995 / 12.051 / 18.959 ms |
| Camera FP32 | 0.240 ms | 192.449 ms | 4.856 / 12.878 / 21.811 ms |
| Portrait FP16 | 0.233 ms | 39.105 ms | 7.631 / 9.559 / 15.630 ms |
| Portrait FP32 | 0.234 ms | 43.245 ms | 10.274 / 14.548 / 18.340 ms |

Each case verifies 1,000 hits share the retained immutable texture: **4,000 total**.
Hits include exact plan preparation, raw-source ownership/interpretation checks,
cache lookup and GPU completion polling, with no new upload/dispatch/readback.
Each case then renders 120 sequential native pictures, **480 total**; the first
source binding/hash/open/render is separated from 119 steady requests. Timing
includes native decode, planning, upload, typed graph work and GPU completion.
No readback occurs on this timed resident sequence. All steady p95 values pass
the predeclared 33.3 ms **picture-kernel** budget; an earlier local candidate reached
a portrait FP32 maximum of 51.263 ms (see `earlier-candidate.json`). These are
ordinal-mapped fixtures, not natural VFR playback,
frame scheduling, native presentation, audio/drop/drift or delivery proof.

Texture cache remains <=512 MiB; the portrait ends at 530,841,600 bytes in 32 FP16
or 16 FP32 entries. Raw pictures remain at 265,420,800 bytes/32 entries, below their
256 MiB limit. Native regressions separately prove externally pinned/in-flight
ownership and exhaustion. Cleanup leaves zero cache/live texture/raw bytes,
entries, pending submissions, source descriptors and decoders. Actual process HWM
is 455,808 KiB camera and 575,248 KiB portrait. These counters and process memory do
not establish a universal 4 GiB/hardware baseline; their exclusions are explicit.

An initialized graph issues an uncached native seek after its first completed GPU
picture and is cancelled after 10 ms. Completion, cleanup and worker join take
**17.750 ms camera / 9.984 ms portrait** after cancellation, within 2 s. Native
seek counters prove issued source work rather than a preflight check/cache hit.
This qualifies cancellation during a native source miss inside the graph; it does
not claim preemption of submitted GPU kernels.

The prior raw empty-cache seek p95 remains 295.999/941.064 ms. This qualification
does not close the 250 ms full native warm-seek gate. GPU surface presentation,
process IPC, sound, masks/HDR, codec delivery, long-duration and additional-hardware
qualification, enterprise/GTM and independent professional jobs remain open.
