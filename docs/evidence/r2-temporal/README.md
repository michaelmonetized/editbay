# Retained temporal evaluation evidence

Linux ARM64 / Apple M1 Pro / Omarchy, Rust/Cargo 1.98.0, 2026-10-04. This qualifies
the compiled temporal dependency of EB-023/024/026. Actual composition pixels,
sound intervals, bounded decoded/GPU output caches, native playback and full R2
release gates remain open. [Issue #9](https://github.com/michaelmonetized/editbay/issues/9)
tracks this slice.

## Artifact and software ownership

The frozen unoptimized Rust lab SHA-256 is
`212d9c676d6a4db8f127d29c3044e4145a08c71531a1a59ee3e1943dd9c4dc08`;
CLI `2ec4bf1ec4bfa1bd734f9f73387b0ff7da6abaad1fa09c06282110112d92b4d2`.
Lockfile `ced164e8dba53bb55bcb933c764a2a2b2a1d8d0865765c06e099148ba82ff064`.
`artifacts.txt`, `elf-header.txt` and `source-hashes.txt` identify the actual
AArch64 binaries and compiled inputs; the containing commit identifies source.
Private binaries and raw receipts are retained under this checkout at
`artifacts/r2-temporal-final/`. The build target is `target/native-build/` and
`TMPDIR` is `artifacts/tmp/`; both parent directories are ignored by Git.

The full default workspace suite passes **96 tests plus five SDK documentation
examples**; all features pass **97 plus five**, with the pinned native libtorch
2.10.0+cpu reference selected. The publisher checksum of the extracted native
headers/libraries matches the existing [runtime pin](../r0-inference/models.json).
No Python process or backend was used. Full all-feature Clippy with `-D warnings`
and formatting pass. `checks.json` records commands and zero failed/ignored tests.
Copied logs have trailing spaces/final empty lines removed for Git; their results
are unchanged. Original logs remain in `artifacts/`.

Seven new core regressions cover published integer-inspection parity, unsigned
time-map endpoints and large fractional arithmetic, different-rate nested picture/
sample/animation time, reverse end/step ownership, freeze/range/sample boundaries,
retained operation reuse and immutable worker version, and required source/mask/
working-gamut/precision/dimension invalidation. Existing real CLI/MCP, media/GPU,
command, save and recovery tests remain passing.

The frozen real CLI produces a **byte-identical** copy of the published child
frame-24 JSON. Its established frame-plan SHA remains
`451a76e94d25277cda2ee25aa01ac7ff38b9ac26662d3609924d6520809040de`.
This receipt describes parameters and source requests, not rendered output.

## Measured planning

The budget was set before measurement: **planning p95 <= 1 ms** for a ten-node
masked/animated picture plus gain graph, alternating direct fractional and
reverse-nested requests. Each case records 10,000 iterations after 100 warmups.
Initialization, import/source indexing and one-shot inspection are separate.
All measurements use the frozen unoptimized lab, owned source descriptors and
full-byte verification before/after qualification.

| Case | Indexed pictures | Initialize | Retained planning p50 / p95 / max | One-shot inspection p95 |
| --- | ---: | ---: | ---: | ---: |
| Actual camera source | 314 | 1.237 ms | 0.263 / 0.284 / 6.901 ms | 1.741 ms |
| Actual screen recording | 11,471 | 13.087 ms | 0.263 / 0.281 / 0.417 ms | 13.326 ms |
| Synthetic index stress | 500,000 | 620.446 ms | 0.263 / 0.282 / 0.434 ms | 620.627 ms |

The real camera source retains SHA-256
`498ec6c42b6b440886eb761dc4775c994f7578021890ceae3f370ffc6fb3e021`
and 5,235,017 bytes; selected video/audio indices are 1/0. The real recording
retains `215a3a547eaa7f776772ba54ef44eae3a32a2cbf0cd0a83f964ebe378bed8762`
and 143,966,323 bytes. The receipts include exact selected streams, document/key
hashes, runtime versions and 20 retained-versus-one-shot inspection comparisons
per case. Peak process HWM is 88,576 KiB for camera plus stress and 57,776 KiB for
the real recording. Source bytes are unchanged; no source pictures are published.

The edit graph is synthetic, referencing the actual imported picture/sound index.
The 500,000 timestamps are separately generated alternating 33/34 tick intervals;
they are **not** actual source pictures or proof of long playback. Planning does
not read/hash the complete timestamp index each frame or copy static polygon
arrays. The large initialization/one-shot costs explain why production must retain
the context and construct it on a worker.

## Remaining production proof

The shared GPU renderer and audio interval engine still need to consume these
plans. Cache keys here describe working image dependencies and sample points;
they do not prove actual output cache eviction, memory budgets or audio-block
identity. Native preview/export parity, bounded caches, scheduling, hardware
handoff, display/HDR color, warm/cold seeks and two-hour drift remain open.
The previous uncached CPU seek p95 still misses the unchanged 250 ms warm budget.
No new native presentation, physical audibility, additional GPU or independent
client-job qualification is claimed.
