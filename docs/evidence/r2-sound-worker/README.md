# Isolated sound qualification

Qualified 2026-10-05 23:59:32–23:59:41 UTC on Linux ARM64, Apple M1 Pro.
Source commit: `e3fc6849e69dcd365d0af66f689f4044a9e3f0cc`, stacked above PR #20.
Issue #21 covers this bounded codec-worker dependency. The packaged GUI and CLI
both host the real Rust PCM endpoint before their normal initialization.

All eight final receipts pass every declared gate. PCM retains native channel
order, float headroom, absolute sample intervals and unchanged source hashes.
The independent reference is sequential FFmpeg f32le without rate or channel
conversion; maximum absolute PCM error is zero in every case.

| Source / endpoint | Handoff p95 (ms) | Preparation p95 (ms) | Unity render p95 (ms) | Sinc p95 (ms) | Combined HWM (KiB), maximum of raw/graph |
| --- | ---: | ---: | ---: | ---: | ---: |
| Camera / app | 0.069292 | 2.465802 | 0.909920 | 27.798783 | 88528 |
| Camera / CLI | 0.046584 | 2.395801 | 0.906337 | 25.702524 | 85056 |
| Six-channel AAC / app | 0.087751 | 2.387969 | 1.030712 | 28.541994 | 90016 |
| Six-channel AAC / CLI | 0.056709 | 2.386093 | 0.956671 | 27.838950 | 88384 |

Budgets remain handoff <=10 ms p95, preparation <=5 ms p95, warmed unity render
<=20 ms p95, sinc <=40 ms p95, first native render <=250 ms, cancellation <=2 s
and combined sound-only parent/child HWM <=512 MiB. Memory is the conservative
sum of process high-water values and can double-count shared physical pages.
Each raw route checks up to 64 distinct 4096-frame blocks and 1000 shared hits.
Each graph route checks the same nested 0.25/0.75 gain mix used by native sound,
100 warmed renders and 30 sinc renders at 44.1 kHz.

Active request cancellation/join/reap is <=3.158221 ms. An intentionally SIGSTOP'd
codec child is killed and reaped in <=102.833721 ms. Actual SIGKILL, explicit
rebind, old-receipt rejection, equal fresh samples, consumer pins after cleanup,
zero final mapped bytes/handles and unchanged original sources pass all routes.
These exercise actual processes and samples, without implying a native playback
control, streaming callback, physical audibility, device latency or long drift.

Local final checks: 149 default tests plus seven documentation examples; 150
all-feature tests plus seven examples, with native libtorch selected; zero failed
or ignored tests. All-feature/all-target Clippy with warnings denied and formatting
pass. Seven new regressions cover independent lossless/AAC/container PCM,
reverse/retimed gain/mix parity, immutable source ownership, pins, cancellation,
private protocol ownership and malformed descriptors/messages. All six existing
picture-worker integration tests pass after extracting common supervision.
The codec library unit tests also exposed and fixed FFmpeg link ordering under
`--as-needed`; the static adapter now precedes its dependent libraries.

## Reproduction and provenance

Set `TMPDIR` under the checkout. Local builds used `CARGO_BUILD_JOBS=2` and
`CARGO_TARGET_DIR=/home/michael/Projects/editbay/artifacts/worktrees/r2-sound-engine/target/native-build`.
Fresh release binaries were copied into ignored `artifacts/qualification/bin/`.
Source, lock, artifact and fixture hashes, native libraries, platform and complete
JSON/log receipts accompany this document. No private media or binary is committed.

```sh
export TMPDIR="$PWD/artifacts/tmp"
env -u RUSTUP_FORCE_ARG0 cargo build --release --locked \
  -p editbay-lab -p editbay-cli -p editbay-app
artifacts/qualification/bin/editbay-lab pcm-worker SOURCE PACKAGED_APP_OR_CLI
artifacts/qualification/bin/editbay-lab sound-blocks-worker SOURCE PACKAGED_APP_OR_CLI
cargo test --workspace --locked
EDITBAY_LIBTORCH=/home/michael/Projects/editbay/artifacts/native-torch/torch \
LD_LIBRARY_PATH=/home/michael/Projects/editbay/artifacts/native-torch/torch/lib \
  cargo test --workspace --all-features --locked
```

Run qualifiers against newly copied binaries after builds and tests finish.
Inspect every JSON `qualified` field as well as operational exit status.

## Earlier runs and limits

Both earlier runs are retained under `earlier-candidates/`, using source
`1cdbe8c3f7a46d2705d824b21e052c2d6309a106` and the recorded artifact hashes. The
contended six-channel CLI graph missed sinc p95 at 44.662767 ms during heavy
parallel-link memory pressure. A second camera CLI run missed preparation p95 at
8.854453 ms. Full receipts and memory-pressure snapshots remain intact; neither
is passing evidence. Their exact binaries remain in ignored
`artifacts/qualification/earlier-candidates/bin/`.

Ordinary fractional time interpolation now reduces once when its checked
intermediate arithmetic fits; extreme ranges retain cancellation-before-product
arithmetic. The full core/time/sound tests pass. Final measurements use that new
source and frozen binaries after build completion. No numeric budget was raised;
no processor affinity or machine configuration was changed.

Natural source-sequence sound timing (#22), callback scheduling, device routing,
two-hour playback/drift, shared delivery, full R2 hardware and later product gates
remain open. Parent and child worker measurements do not qualify those routes.
