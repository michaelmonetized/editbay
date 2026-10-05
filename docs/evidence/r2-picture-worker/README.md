# Retained codec process and sealed picture evidence

Linux ARM64 / Apple M1 Pro / Omarchy, Rust/Cargo 1.98.0, local 2026-10-04.
Qualification UTC interval: 2026-10-05 02:43:10–02:44:21.
[Issue #15](https://github.com/michaelmonetized/editbay/issues/15) tracks this
dependency of EB-022/025/026. [Contracts](../../WORKER_PICTURES.md) define ownership,
limits and supported operations. Native surface/audio/delivery and full R2 remain open.

## Frozen artifacts and checks

The unoptimized AArch64 lab SHA is
`d385985a99d4776f867d014586a2dfb2d619fb91cc8ea38d6bd2b713d90abf20`.
Actual native app child SHA:
`40a52481c69616f9f3df8077be3552ca9c5be345d3c17daf2bde04c2c98d9deb`;
actual CLI child SHA:
`ae321d7b69428d40b6da10cf0274f6fd5674339f30c3fab64d4e0e615513e87d`.
Lockfile `c96ece330591b36c3537e8e72eff6fce15ab7f35e6ae9322d0f36d6ca6dbd6df`.
`artifacts.txt`, `elf-header.txt` and `source-hashes.txt` identify binaries and
selected compiled inputs. The containing commit identifies complete source.
Private binaries/original logs remain in `artifacts/r2-picture-worker/`, earlier
candidate artifacts in `artifacts/r2-picture-worker-initial/`. Builds and temporary
fixtures stay in `target/native-build/` and `artifacts/tmp/`. Both roots are ignored.
No raw footage, pictures, weights or binaries are committed.

The default workspace passes **119 tests plus five SDK documentation examples**;
all features pass **120 plus five**, with pinned project-local native libtorch.
Zero failed/ignored. All-feature/all-target Clippy `-D warnings` and fmt pass.
`checks.json` and logs record gates. Ten new regressions exercise actual two-stream
VFR H.264 with delayed pictures, exact reverse/after-EOF selection, kernel seals,
descriptor lifetime/CLOEXEC, extra/truncated ancillary cleanup, malformed/oversize
framing, private foreign/stale session/job/worker/version/generation/serial checks,
independent byte/handle pin exhaustion, SIGKILL/retry, SIGSTOP cancellation, changed
source preservation and identical in-process/process GPU evaluation. Editing a stale
receipt's public version/generation does not revive it.
The foundation/CLI/MCP, recovery, native ingest/GPU/audio-clock regressions remain
passing. This slice enables no new GUI controls and claims no new window trial.

## Real sources and plane transfer

Camera: stream 1, 314 indexed pictures, 1280x720, 5,235,017 bytes, SHA
`498ec6c42b6b440886eb761dc4775c994f7578021890ceae3f370ffc6fb3e021`.
Portrait: stream 0, 528 pictures, 1080x1920, 777,868 bytes, SHA
`872e2e7f3cb19ff13653bea9784c48d78969ceb4b8f7da18d1ea5c7cbaa9a48f`.
The camera uses the actual **app** codec endpoint and portrait the actual **CLI**
endpoint. Source descriptors/checksums remain read-only before/after every run.
Twenty-five targets per source match complete independent sequential native RGBA
checksums and actual color/alpha metadata. Reverse at the exclusive source end
selects the final verified picture. The first-picture checksums match the prior
raw/GPU reference receipts. Originals remain intact.

The predeclared **already-decoded handoff budget is p95 <=10 ms** for 1080p RGBA.
The portrait transfers 8,294,400 bytes, equal to a 1920x1080 RGBA payload; camera
transfers 3,686,400 bytes. Each run performs 1,000 hits through real IPC, native
identity checks, envelope/seal validation and mapping lookup. Every hit shares the
verified immutable `Arc`, with no new native decode, pixel hash/copy or JSON pixels.

| Source | Process bind | IPC handoff p50 / p95 / max | Source miss p50 / p95 / max | Steady sequential p50 / p95 / max |
| --- | ---: | ---: | ---: | ---: |
| Camera | 16.617 ms | 0.511 / 1.734 / 12.493 ms | 174.970 / 254.661 / 262.390 ms | 5.002 / 10.114 / 26.062 ms |
| Portrait | 17.547 ms | 0.470 / 1.151 / 3.408 ms | 177.034 / 324.550 / 325.139 ms | 5.213 / 8.836 / 20.651 ms |

Misses include the first full source binding/hash/open and later retained-decoder
seeks. OS page caches remain warm; this is not physical cold-disk proof. The 120
sequential pictures per source separate first restart/binding (212.451/57.725 ms)
from the 119 steady requests. Both miss p95 values still exceed **250 ms**.

Before cleanup each route retains 25 sealed planes: 92,160,000 / 207,360,000 bytes
in parent and child counters. They can refer to the same backing pages; adding
those counters or RSS does not describe unique physical memory. Consumer pins
remain charged through eviction/clear/death and exhaust handles independently
of bytes. Child process snapshots show 32–79 descriptors, within its 512 limit.
Actual parent HWM is 151,840 / 292,336 KiB; sampled child HWM 62,816 / 77,536 KiB.
Shared file backing need not appear in RSS until mappings are touched. Payload,
native/process/GPU overhead and the full 4 GiB baseline remain separate evidence.

## Failure, cancellation and identical GPU evaluation

Actual SIGKILL causes a visible failure, reaps the codec and rejects its receipt.
Rebinding a metadata revision starts a new child, retains matching parent pixels
and validates fresh ownership. Camera/portrait error latency is 4.480/2.921 ms;
restart plus verified first-picture retry is 223.543/66.163 ms.
Cancelling an outstanding first-binding/uncached IPC request after 10 ms joins and
reaps in **29.416/18.164 ms**; its precise native subphase is not instrumented.
SIGSTOP fault injection proves the unresponsive-child EOF-grace/kill fallback in
**104.839/104.413 ms**. Both satisfy <=2 s. All mapped bytes/handles, child sources,
decoders, owned entries and GPU submissions/textures return to zero at cleanup.

The established five-node ordinal-mapped source/solid/scalar/opacity/over fixture
uses this exact provider and shared temporal/GPU implementation. Three pictures
per source/precision compare every channel with independent native decode plus
CPU transfer/alpha expectations. Maximum linear error is 0.000418425 FP16 and
0.000000119209 FP32, within the existing 0.002 / 0.00002 limits. Encoded output
agrees within 0.004 / 0.00004. Actual adapter remains Apple M1 Pro (G13S C0), Vulkan,
Honeykrisp Mesa 26.2.3-arch1.1.

| Case | GPU hit p95 | First restart/binding picture | Steady completed-picture p50 / p95 / max |
| --- | ---: | ---: | ---: |
| Camera FP16 | 2.366 ms | 217.852 ms | 9.804 / 21.625 / 31.470 ms |
| Camera FP32 | 2.940 ms | 238.480 ms | 11.158 / 23.983 / 41.666 ms |
| Portrait FP16 | 2.703 ms | 94.996 ms | 19.315 / 25.242 / 33.816 ms |
| Portrait FP32 | 2.525 ms | 96.434 ms | 23.543 / 28.276 / 32.991 ms |

All four scoped **completed-picture p95 <=33.3 ms** cases pass, with 480 pictures
and 4,000 shared resident hits, no new hit upload/dispatch/readback and no timed
sequence readback. Parent graph HWM is 436,000 / 532,608 KiB. These 119-steady-sample
kernel receipts do not claim zero drops or frame maxima <=33.3 ms. An initial
portrait FP32 candidate reached p95 **35.506 ms**, failing this gate; its full
receipt and binary hashes are retained in `earlier-candidate.json`. The final path
decodes directly into sealed outputs and avoids intermediate seek conversion.

Native display surfaces, frame scheduling/drop policy, sound intervals/clock,
two-hour drift, shared encoder delivery, masks/HDR, image sequences, other hardware
and independent production/GTM jobs remain open. This handoff qualifies one R2
dependency; it does not qualify the full release.
