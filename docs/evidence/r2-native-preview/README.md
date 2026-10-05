# Native shared-device picture workspace evidence

Linux ARM64 / Apple M1 Pro / Omarchy, Rust/Cargo 1.98.0, local 2026-10-05.
Native qualification UTC interval: **04:12:58–04:25:42**.
[Issue #17](https://github.com/michaelmonetized/editbay/issues/17) tracks this
dependency of EB-023/024/025/026, following PR #16. The
[contract](../../NATIVE_PREVIEW.md) defines actual behavior and limits. This
qualifies paused native pictures and their document/recovery path; full R2 and
enterprise/GTM gates remain open.

## Frozen artifacts and software

All four picture cases, fresh UI ingest and the new 100-trial recovery regression
use the **same** unoptimized AArch64 app:
`d23d835f4b7d2e9f12a66cea4e42881efc75bb429ff16d6f7b0d4a9fb4178fd9`.
The Rust driver SHA is
`866022f7fb0f6c5009f571f05f4890b8a633f2be02c2399339937cb92eac6518`;
CLI SHA `c3ee115b462424e280495b1fab99046bbd7e3771d651efa6e1ab4d157af431c8`.
Lockfile `e99669e2830ae3ba89cd81031896fa7323779f69a05707720b83b9ced34f4b09`.
`artifacts.txt`, `elf-header.txt` and `source-hashes.txt` identify exact binary and
compiled inputs. Source hashes were checked again against the final checkout.
Private binaries, raw JSONL traces, projects and logs remain in
`artifacts/r2-native-preview-qualified/`; failed/development candidates remain in
the separate `r2-native-preview-initial/`, `r2-native-preview/` and
`r2-native-preview-final/` directories. All project/build storage stays in this
checkout, under ignored `artifacts/` and `target/`.

Default workspace checks pass **126 tests plus five documentation examples**;
all features pass **127 plus five**, with the verified project-local native
libtorch. Zero failed/ignored. All-feature/all-target Clippy `-D warnings`, fmt
and local workspace build pass. `checks.json` and logs retain results. Seven new
tests exercise source-sequence authoring, VFR/partial tails, one-group history,
stale background authoring and same-pass UI revision changes, plus actual GPU
surface pixels, foreign ownership and submitted/unsubmitted allocation lifetimes.
The existing core/CLI/MCP/ingest/recovery/codec/GPU/clock checks remain passing.

GPU surface tests cover both float precisions and all four RGBA/BGRA linear/sRGB
formats. Every output pixel compares with an independent orientation, bilinear,
alpha/checkerboard and transfer expectation within **1.2 output code values**;
alpha is opaque. Encoded commands retain charged picture memory before submit;
actual completion or dropping an unsubmitted command releases it. Foreign device,
worker and undeclared display-boundary receipts fail. Normal preview performs
zero pixel readbacks. The actual window surface is `Bgra8Unorm`.

## Native picture cases

Commands use the frozen binaries:

```sh
editbay-lab native-preview editbay-studio CAMERA NEW_DIRECTORY full
editbay-lab native-preview editbay-studio CAMERA NEW_DIRECTORY half
editbay-lab native-preview editbay-studio PORTRAIT NEW_DIRECTORY full
editbay-lab native-preview editbay-studio PORTRAIT NEW_DIRECTORY half
```

Camera: stream 1, 314 indexed pictures, 1280x720, SHA
`498ec6c42b6b440886eb761dc4775c994f7578021890ceae3f370ffc6fb3e021`.
Portrait: stream 0, 528 pictures, 1080x1920, SHA
`872e2e7f3cb19ff13653bea9784c48d78969ceb4b8f7da18d1ea5c7cbaa9a48f`.
Native source indexing seeds the project; the **actual UI** creates its editable
composition/sequence through a normal undo group. Each run steps between frames
0/1 forty times, records every injection/acceptance/completed draw, and excludes
only the first two cache-filling requests from its 38 cached samples. The scoped
budgets stay **input p95 <=50 ms**, **cached request-to-draw p95 <=250 ms**, and
**active cancel <=2 s**.

| Case | Input p50 / p95 / max | Cached draw p50 / p95 / max | Active cancel |
| --- | ---: | ---: | ---: |
| Camera FP32 | 19.665 / 29.932 / 43.292 ms | 29.364 / 38.215 / 38.645 ms | 137.680 ms |
| Camera FP16 | 20.059 / 33.257 / 36.839 ms | 30.523 / 36.816 / 37.482 ms | 131.641 ms |
| Portrait FP32 | 18.835 / 30.375 / 35.267 ms | 29.469 / 34.878 / 38.549 ms | 124.998 ms |
| Portrait FP16 | 21.262 / 28.926 / 30.629 ms | 29.368 / 36.776 / 37.977 ms | 132.635 ms |

All pass. These are software-injected native input and exact submitted-GPU-draw
completion timings, including UI scheduling. They do not measure physical input,
display photons, sustained transport or frame drops. Largest sampled parent HWM
is **152,352 KiB**. Resident/source counters and zero normal-preview readbacks
are retained in each receipt. This is not a full 4 GiB production-workload proof.

The driver stops the actual codec, scrubs ahead through real UI input, resumes
it and requires superseded-result rejection. SIGKILL is noticed while idle;
the native error and Retry control are inspected in `worker-failure.png`.
Retry starts a different process. An outstanding stopped-codec request is
cancelled through the actual viewer button; its process is reaped and the seed
document remains intact. Native Undo/Redo, Save, revision-4 checkpoint, process
kill, welcome recovery through the current Flea chooser and original-file reopen
all retain the authored graph and original sources. Recovery creates a distinct
project identity and preserves saved original/checkpoint hashes.

The committed 800x600/1440x900 camera/portrait, recovered and error captures were
inspected. The toolbar wraps at small sizes; frame and cancel/retry controls stay
visible, with the document/recovery area scrollable. The current Omarchy palette
is retained in `omarchy-colors.toml`. GPU/backend remains the previously qualified
M1 Pro Vulkan/Honeykrisp route; other hardware is not inferred from it.

## Fresh ingest and recovery foundation

`editbay-lab native-media editbay-studio CAMERA NEW_DIRECTORY` uses the actual
current Flea chooser, explicit stream selection, worker death, active cancellation,
Undo/Redo, checked Save and independent recovery. It passes with **1,135.885 ms**
import and **148.309 ms** cancellation. Original picture/sound stream records,
source bytes and immutable checkpoint hashes survive. No listening or sound
playback is claimed by an ingest receipt.

`editbay-lab native-workspace editbay-studio NEW_DIRECTORY 100` passes **100/100**
new kill/recover/reopen trials with 4,000 catalog documents, 50 saved originals and
50 untitled projects. Active/inactive acknowledged checkpoints survive; recovery
has independent identity; original/checkpoint hashes remain intact. There are
250 native edits: input p50 **13.849 ms**, p95 **29.030 ms**, max **45.946 ms**.
Two hundred main-thread checkpoint publications measure p95 **3.534 ms**, max
**21.188 ms**. Sampled parent HWM max is **132,928 KiB**. CPU UI-frame work has
p95 **4.917 ms**; startup maxima are retained rather than excluded.

## Retained failures and remaining work

`earlier-candidates.json` preserves the initial completed camera run that missed
input p95 at **51.274 ms**. The corrected driver declares its 8 ms button-event
delay and timestamps actual UI enqueue separately from later diagnostic state.
The 50 ms budget was preserved. Fullscreen/control/portal/pagination/selection
driver failures and an intentionally interrupted 39-trial older-artifact regression
are retained separately; none counts as a passing final trial. A new same-pass
revision check invalidates the viewer before painting after an edit.

Sound intervals/clock, sustained playback/drop/drift, full warm/cold uncached seek,
shared encoder delivery, masks/HDR, image sequences, hardware adapters/matrix,
complete professional jobs, install/update/rollback and independent GTM use remain
open. Earlier 254.661/324.550 ms uncached source-seek failures still apply; these
paused cached-picture passes do not replace that gate.
