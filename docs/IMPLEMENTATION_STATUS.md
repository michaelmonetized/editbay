# Rust restart: implementation status

Validated locally on 2026-10-04, Linux ARM64, Rust/Cargo 1.98.0.
This is the native local workspace, project/recovery foundation and native
engine/inference feasibility tools. The local R1 workspace gates pass; timeline
authoring and complete-job release gates remain open.

## Implemented

- Cargo workspace with `editbay-core` and the `editbay` CLI, a generated
  dependency lockfile, structured errors, and no production Python dependency.
- Fresh validated document schema: stable UUIDs, revisions, sequence dimensions,
  rational frame rates, duplicate-identity checks, and explicit schema rejection.
- Project creation/load/rename/save. Writes synchronize the file and parent
  directories; existing saves publish atomically. New project/checkpoint publication
  refuses an existing destination without an overwrite race.
- Persistent writer locks and optimistic concurrency for CLI edits. A stale
  loaded document cannot replace newer work or recreate a deleted project.
- Immutable recovery checkpoints with identity/revision/path/time metadata and
  SHA-256 integrity verification. Untitled projects can be checkpointed through
  the core API. A broken latest checkpoint leaves older valid work discoverable.
- Recovery to a new independent project with an origin ID. Recovery refuses the
  original destination and existing files, including when the source was deleted.
- The active `bin/editbay` builds/runs Rust. Application source and tests are
  confined to the Cargo workspace.
- Native `editbay-media`, `editbay-render`, `editbay-audio` and `editbay-lab`
  prototype decode, explicit FP16/FP32 GPU color/alpha composition, device-clock
  sound playback, and an isolated cancellable FFV1 picture export worker.
  Export reopens and checks every decoded picture byte before separate-file
  publication; source changes and destination conflicts fail explicitly.
- Predeclared enterprise/GTM budgets in `RELEASE_CRITERIA.md`; actual ARM64
  measurements, hashes and limits in `evidence/r0-engine/`.
- Verified native SAM 2.1 full video-memory propagation and RVM human-matting
  candidates in `editbay-ai`, with original-coordinate preprocessing/output,
  source/configuration-owned snapshots, strict recurrent geometry and active
  ONNX operator cancellation. Failed work cannot advance state.
- Independent RVM official TorchScript comparison through optional native libtorch;
  independent SAM graph kernels through optional Tract. Runtime/artifact/license
  pins and actual receipts live in `evidence/r0-inference/`. No weights are bundled.
- Codec output owns a file descriptor. Existing media, symlink targets and a
  replacement pathname survive internal encoder calls.
- Typed atomic command groups, expected project/revision checks and bounded
  undo/redo with monotonically increasing revisions. CLI rename uses this path.
- Native scoped `editbay-mcp` with actual document inspection, rename groups,
  undo/redo, checkpoint and recovery inspection. Successful edits are durably
  saved before session state/history changes; outside edits and deletion fail.
- Local source inventory: 41 actual client videos and 65 stills, hashes and
  first decoded video pictures; fixed native/interchange fixture inputs with
  ownership, expected outcomes and explicit migration limits.
- Native `editbay-studio` using the pinned eframe/egui 0.36.1 and wgpu 30 stack,
  Omarchy palette/font updates, Phosphor icons, three-column welcome and a compact
  layout. Bounded incremental catalogs find deeply nested work and shared markers,
  with search, profile filters, natural-aspect empty sequence cards and multi-open.
- Independent project tabs, rational picture profiles, core rename/undo/redo,
  checked saves, save-as/copy, close guards, checkpoint history and recovery previews.
  Untitled and inactive tabs use worker preparation and ownership-checked publication.
- Atomic startup/workspace/shortcut settings; saved-tab restoration without login;
  native portal dialogs on Tokio workers with captured document/bank ownership.
- Original-byte shared brand imports/exports, palettes and client/project metadata.
  EditBay's versioned namespace preserves Omadesign manifests and legacy markers.
  Fonts/title packages/LUTs are stored assets; their execution belongs to authoring.
- Schema 2 assets/streams, exact source tick mapping, typed tracks/clips and nested
  compositions, timed image/mask/geometry/audio/data nodes, channels and separate
  working/display/output color settings. Graph/socket/identity/link validation,
  reversible entity commands and transitive semantic fingerprints share one core.
- In-memory schema 1 migration, legacy checkpoint integrity before upgrade,
  separate-file format migration, CLI command groups/frame-plan inspection and
  revision-owned MCP frame-plan inspection. Native tabs retain structured documents;
  shared immutable snapshots keep frame/file handoffs independent of graph size.
- Native/CLI media ingest with explicit installed-decoder stream selection,
  read-only descriptor/full-checksum ownership, decoded VFR picture indices,
  original-channel float sound and source timecode/reel/color/alpha metadata.
  Source import uses normal undo/redo/save/recovery. Exact picture seeks flush
  delayed decoders and work after EOF; single PNG/JPEG sources retain alpha.
- Packaged Rust codec child with typed bounded pipes, native cancellation,
  memory/CPU/handle limits, secondary-resource denial, visible crash recovery
  and session/revision-owned off-thread document preparation. Native camera
  import, active cancellation, worker death, undo/save and independent recovery
  are exercised through the actual window and portal.
- Immutable `EvaluationSnapshot` compiles source interpretations, reachable
  inputs and shared static operations once. Fractional/reverse nested time,
  step-animation boundaries and native picture/sample selection share the integer
  inspector. Working-content keys include source/mask bytes, dimensions, color,
  precision and dependencies; document version still controls publication.
  This is temporal planning, not rendered pictures or sound-block evaluation.
- Source-owned native decoded-picture cache: exact indexed requests, retained
  sequential decoders, bounded cache/live RGBA payload and handle limits, LRU
  eviction, charged consumer pins, fresh-token version/generation rebinding and
  full source verification. Real camera/portrait pixels, shared hits, cleanup and
  active seek cancellation are qualified. Native IPC/GPU/presentation remain open.

## Validation

| Check | Result |
| --- | --- |
| `cargo test --workspace --locked` | 101 tests and 5 SDK documentation examples passed; 0 failed or ignored |
| `cargo test --workspace --all-features --locked` | 102 tests and 5 SDK documentation examples passed with native libtorch selected; 0 failed or ignored |
| Core regression suite | 23 tests: identity/rational time, validation, round trips, permission preservation, concurrent/stale writers, corruption, source loss, bounded reads, unpublished preparation/cleanup and overwrite refusal |
| CLI binary integration suite | 3 tests: complete recovery flow, command failures and real process interruption during save; paths contain spaces |
| Native codec/GPU/worker/clock tests | 8 tests: lossless pictures and rational time; delayed video/audio drain; invalid codecs; output descriptor/path ownership; actual FP16/FP32 GPU parity; export/cancel cleanup; device-clock interpolation |
| Inference boundary tests | 4 tests: bounded finite tensors, preflight cancellation, forged SAM memory/prompts, source-coordinate resampling and incompatible RVM state geometry |
| Commands/reference/automation | 8 tests, including 4 already counted in core: atomic groups, monotonic undo/redo, owner/stale/overflow checks; immutable native fixture replay; real MCP process workflows under both protocol generations; outside edits/deletion; competing servers; source inventory/error/no-overwrite behavior |
| Native workspace/bank | 16 tests: incremental deep catalogs and cancellation, palette parsing, settings concurrency/corruption, exact asset round trips/conflicts, stale recovery/save ownership, inactive recovery, bank draft merging and native input/focus regressions |
| Schema 2 document | 8 additional core tests: editable job commands/undo/save/recovery, pinned legacy checkpoint migration/integrity, invalid graph/socket/link/identity rejection, exact VFR/retiming/sample boundaries, animation/history ordering, semantic frame fingerprints, immutable snapshot ownership and actual 64 MiB read/write limits |
| Typed CLI/MCP | 2 additional real CLI workflows and 1 real MCP process workflow: authored composition/frame inspection, stale rejection, undo/redo/recovery and separate-file migration |
| Native ingest/worker | 11 additional tests: actual VFR/multiple streams, exact backward seeks after EOF, original six-channel order/float headroom, PNG/JPEG alpha, AAC priming, source replacement, native cancellation/secondary-resource refusal, packaged child resource limits/crash/stale ownership, CLI durable ingest/recovery and document-read budget |
| Vendored native SDK | 9 focused Wayland redraw and clipboard/modifier tests passed |
| R1 native kill/recover/reopen | 100/100 trials passed with 4,000 catalog documents; 50 untitled and 50 saved originals; active/inactive acknowledged checkpoints retained, independent recovery identities and unchanged source/checkpoint hashes |
| R1 native input/publication | 250 inputs: p50 11.768 ms, p95 25.086 ms, maximum 65.256 ms; 200 UI checkpoint commits: p95 1.059 ms, maximum 4.585 ms; peak RSS/HWM 103,248 KiB |
| R2 document native regression | 100/100 new native recovery trials; 250 inputs p95 22.256 ms, maximum 33.244 ms; 200 UI checkpoint commits p95 0.564 ms, maximum 4.204 ms; peak RSS/HWM 101,360 KiB. Authored graph Save/checkpoint/recovery and legacy profiles inspected; source hashes preserved |
| R2 native camera ingest | Actual portal/stream selection, worker SIGKILL with idle UI error, active cancellation 126.238 ms, import 2,284.802 ms, undo/redo, Save revision 3, checkpoint/recovery revision 4 with separate identity and preserved source/stream hashes; parent peak HWM 129,696 KiB |
| R2 ingest native recovery regression | 100/100 fresh kill/recover/reopen trials; 250 inputs p95 26.730 ms, maximum 88.814 ms; 223 UI checkpoint commits p95 0.928 ms, maximum 3.951 ms; peak parent HWM 124,304 KiB. Sources/originals/checkpoints retained; incomplete driver runs are separately recorded |
| Real indexed picture/sound | Portrait: 528 pictures, 25 pixel-equal seeks, CPU seek p95 631.905 ms, HWM 89,184 KiB. Camera: 314 pictures, 26 pixel-equal seeks, 627,040 native mono 48 kHz samples, CPU seek p95 469.515 ms, HWM 64,368 KiB. Both miss the 250 ms warm-seek gate; no cache/playback qualification claimed |
| Retained temporal planning | 10,000 evaluations per case, ten child nodes plus reverse-nested parent: camera index 314 pictures p95 0.284 ms; actual recording index 11,471 pictures p95 0.281 ms; synthetic 500,000 timestamps p95 0.282 ms. All pass the separate 1 ms CPU planning budget. Initialization and one-shot inspection remain off the frame path; no rendered output/cache/playback claim |
| Decoded-picture cache | 50 pixel-equal real seek targets; 2,000 shared immutable hits. Camera/portrait hit p95 0.005/0.003417 ms; empty output-cache miss p95 295.999/941.064 ms still exceeds 250 ms. 120 sequential pictures per source p95 5.637/3.727 ms, including first binding separately in maximums. Active seek cancellation 8.689/3.381 ms; payload/handle cleanup zero; HWM 325,856/347,536 KiB. Raw CPU output only; full native seek/playback gate remains open |
| Native recovery timing | 23 continuous edits: first durable publication 9,807.159 ms, latest idle publication 1,051.024 ms; input acceptance p95 16.844 ms; 1 s idle debounce with <= 250 ms publication allowance |
| Native storage failures | Actual EACCES and ENOSPC errors displayed; no false checkpoint acknowledgement; retry published a valid checkpoint after repair |
| Native layout/input/assets | Final 800x600 and 1440x900 dark/light welcome captures inspected; native portrait profiles, Fcitx IME, Ctrl+V/Shift+Insert, preferences and repeated portal imports/exports inspected; exact SVG/font byte round trips and unsaved-bank close guard |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed with native libtorch selected |
| Optional reference features | Local all-feature test suite and Clippy pass with verified native libtorch selected; actual reference model runs are separate measured receipts |
| `cargo fmt --all -- --check` | Passed |
| `./bin/editbay --version` | `editbay 0.1.0 (Rust project foundation)` |

The tests exercise actual local files, CLI processes, native codec libraries and
the M1 Pro Vulkan GPU. A real save process was killed and restarted successfully.
The actual ALSA callback route and short playback scheduling were measured.
Native-window recovery and input performance passed all 100 qualification trials;
timing, storage-error and inspected native workflows have separate receipts in
[R1 evidence](evidence/r1-workspace/README.md). Power loss, physical audibility/latency,
other advertised hardware and live services remain unqualified.
Schema 2 software, actual CLI/MCP workflows and the fresh native 100-trial
regression have separate [R2 document evidence](evidence/r2-document/README.md).
This does not close production media-engine gates.
Actual native ingest and source/seek receipts are in
[R2 media evidence](evidence/r2-media/README.md). Native/CLI ingest is implemented;
EB-021 and the full R2 release gate remain open.

## Still to implement

Checkpoint pruning, native timeline gestures, image-sequence ingest, production playback,
audio graph, managed GPU/display/output color, delivery, interchange, cloud, and
production inference integration remain roadmap work. The measured picture exporter and
short callback-clock probe are functional feasibility tools, with recorded limits.
SAM 2.1 and RVM native prototypes are measured; production quality, GPU routes,
model-pack distribution and editable cached-result workflows remain unqualified.

R0 includes EB-001–003 and measured ARM64 EB-004/005 prototypes. The inference
comparison gates pass on the documented fixtures. SAM's reference uses independent
Rust graph kernels with shared orchestration; upstream PyTorch video orchestration
and annotated client quality remain explicit production gates. EB-006 records
source/jobs and fixed fixtures; actual interchange conversion is EB-034/054/083
work. EB-007 specifies the complete automation contract and implements only real
foundation commands. R0's persistence and documented ARM64 feasibility gates pass.
Other hardware, physical paths, production jobs and model distribution remain
their later milestone gates. R1's native workspace is implemented and locally
qualified. R2's document and selected native ingest layers are implemented; shared
production picture/sound evaluation, cache/scheduling and media engine qualification
and R3–R11 remain roadmap work. No enterprise or GTM completion
is declared.
