# Bounded native sound qualification

## Final source verification

Rebuilt and qualified on 2026-10-05 at 22:48 UTC from `a0decfa` after the
private-payload ownership fix. Both actual sources pass every declared gate,
including independent PCM comparison with maximum error zero. Camera/six-channel
preparation p95 is 2.999561/2.900520 ms; warmed unity render p95 is
0.492753/0.518545 ms; sinc render p95 is 26.292216/27.739227 ms.
High-water memory is 50592/56560 KiB. Full JSON, source and artifact hashes,
and fresh default/all-feature/Clippy logs are in [final receipts](final/).
The final frozen binary is `artifacts/sound-qualified/final/editbay-lab`.
Default tests pass 142 tests plus seven documentation examples; all features
pass 143 tests plus seven examples. Formatting and all-feature Clippy pass.
The earlier measurements below remain intact for provenance.

Qualified 2026-10-05 10:59:55–11:01:01 UTC on this Linux ARM64 Apple M1 Pro.
The source commit is `8449497219664bf4602a3aac82a138ef07fd8a0d` on the
sound-engine layer above native preview PR #19. Production sources were already
built at `06c665d`; the second commit adds the nonzero-origin regression only.
Frozen lab SHA-256:
`691d276311de7a25ea56f9b90371d1629c0b4c2248bbfe191b97259562a0cfdc`.
Lock SHA-256:
`c34a48a324973839413d1803befce2c4d9b6dbfe2b430c21f4ea6621398717df`.

The binary remains in this worktree's ignored
`artifacts/sound-qualified/bin/editbay-lab`. Artifact/source manifests and native
library/platform records are beside this document. Rust/Cargo builds and tests
run locally. This is sound-only worker evidence, not UI, physical audio, a
streaming callback, a two-hour drift test, process isolation or complete delivery.

| Gate | Declared bound | Actual camera | Six-channel AAC |
| --- | ---: | ---: | ---: |
| 4096-frame nested two-contribution preparation p95 | 5 ms | 2.928633 ms | 4.080969 ms |
| Warmed unity render p95, 100 blocks | 20 ms | 0.458334 ms | 0.504293 ms |
| 44.1 kHz sinc render p95, 30 blocks | 40 ms | 24.984315 ms | 27.683654 ms |
| First native PCM render | 250 ms | 56.219727 ms | 5.218180 ms |
| Active cancellation/join/cleanup | 2000 ms | 0.133042 ms | 0.086751 ms |
| Process high-water memory | 262144 KiB | 50592 KiB | 56976 KiB |
| Independent original-channel PCM maximum error | 0.000001 | 0 | 0 |

Preparation maxima remain recorded: 7.038310/6.314099 ms. Sinc maxima include
first native binding: 59.984987/28.977449 ms. These are p95 gates, not worst-case
callback deadlines. Decode work, filters, gains and mixing remain off callbacks.

Actual camera source: `/home/michael/Downloads/iCloud Photos/IMG_0001.MP4`,
SHA-256 `498ec6c42b6b440886eb761dc4775c994f7578021890ceae3f370ffc6fb3e021`.
The selected original mono 48 kHz stream contains 627040 samples. The graph
covers 12 whole natural seconds; 64 distinct 4096-frame blocks are checked.
The six-channel fixture covers four seconds, 192000 samples, 46 distinct blocks.
Its full source hash is recorded in `fixture-hashes.txt` and its qualification.
No fractional sound tail is compressed into the rounded video duration.

Both graphs mix a direct 0.25-gain path with a 0.75-gain 60 fps nested path under
a 24 fps root. The independent FFmpeg executable decodes native original-rate,
unchanged-channel f32le without filtering. Every rendered channel/sample in the
qualified unity blocks is compared. Exact source checksums are verified again
before successful publication. Foreign compilers/results, cleanup-invalidated
receipts, retained output pins and zero native-handle cleanup pass. Active
cancellation occurs after source warm-up and during a real sinc render on an
owned Rust thread, followed by join and explicit cache/decoder cleanup.

Sixteen additional regressions cover exact source centers and reverse endpoints,
fractional rates and final partial composition samples, inactive/cut/freeze
silence, step/linear/Hermite gain, nested rates, finite gain overflow, channel
conversion rejection, plan budgets/private ownership, actual lossless/AAC/Matroska
PCM, original float headroom, consumer pins, cancellation/source replacement,
nonzero container origins, analytic passband/alias rejection and precision at
large absolute source origins. The actual callback/device path is subsequent work.

## Reproduction

All temp/build/evidence files stay under this checkout. Set `TMPDIR` before
running tests or native tools. Do not overwrite a user source.

```sh
mkdir -p artifacts/tmp
export TMPDIR="$PWD/artifacts/tmp"
export CARGO_TARGET_DIR="$PWD/target/native-build"
export CARGO_INCREMENTAL=0
env -u RUSTUP_FORCE_ARG0 cargo build --release --locked -p editbay-lab
artifacts/sound-qualified/bin/editbay-lab sound-blocks \
  '/home/michael/Downloads/iCloud Photos/IMG_0001.MP4'
ffmpeg -v error -f lavfi -i \
  'aevalsrc=1.25*sin(2*PI*137*t)|0.7*cos(2*PI*263*t)|0.25*sin(2*PI*701*t)|0.1*cos(2*PI*67*t)|0.3*sin(2*PI*997*t)|0.4*cos(2*PI*1103*t):s=48000:d=4:c=5.1' \
  -c:a aac artifacts/sound-fixtures/delayed-51.m4a
artifacts/sound-qualified/bin/editbay-lab sound-blocks \
  artifacts/sound-fixtures/delayed-51.m4a
cargo test --workspace --locked
EDITBAY_LIBTORCH=/home/michael/Projects/editbay/artifacts/native-torch/torch \
LD_LIBRARY_PATH=/home/michael/Projects/editbay/artifacts/native-torch/torch/lib \
  cargo test --workspace --all-features --locked
```

The generated fixture destination must be new. FFmpeg refuses an existing file.
The frozen qualifier returns all individual gates and an aggregate `qualified`.
Its process exit still reports operational failure separately from numeric misses;
check the JSON gates before treating a run as qualified.

## Earlier candidates

`earlier-candidates/candidate-1/camera.json` missed preparation at 6.201904 ms.
JSON serialization of resolved samples was replaced with one fixed-width binary
block hash. Candidate 2 passed the camera at 4.142802 ms but missed six-channel
preparation at 5.474806 ms. Exact ordinary sample-unit conversion, a 64-bit GCD
fast path and one shared output-center calculation reduced preparation overhead.
Their exact binaries remain under ignored `artifacts/sound-qualified/earlier-candidates/`;
artifact hashes and failed receipts are committed here. Those source states were
uncommitted, so only the final candidate has a complete retained source manifest.
No numeric budget was raised.

A focused AAC backward-seek test initially measured 0.000013947487 error after
native decoder flush. Reopening from the presentation start resets complete codec
state and retains the existing 0.000001 comparison bound. Original container-time
seeking and nonzero origins are separately tested. Arbitrary long lossy-codec
seeks remain part of subsequent sustained playback qualification.

Streaming callback scheduling, source-sequence natural sound tails, audio codec
process containment, actual routes/devices, long drift, shared delivery and all
remaining R2/enterprise/GTM gates remain open. The parent project/recovery and
native picture workspace remain on the same typed document/command path.
