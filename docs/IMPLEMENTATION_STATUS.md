# Rust restart: implementation status

Validated locally on 2026-10-03, Linux ARM64, Rust/Cargo 1.98.0.
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

## Validation

| Check | Result |
| --- | --- |
| `cargo test --workspace --locked` | 67 tests and 5 SDK documentation examples passed; 0 failed or ignored |
| Core regression suite | 23 tests: identity/rational time, validation, round trips, permission preservation, concurrent/stale writers, corruption, source loss, bounded reads, unpublished preparation/cleanup and overwrite refusal |
| CLI binary integration suite | 3 tests: complete recovery flow, command failures and real process interruption during save; paths contain spaces |
| Native codec/GPU/worker/clock tests | 8 tests: lossless pictures and rational time; delayed video/audio drain; invalid codecs; output descriptor/path ownership; actual FP16/FP32 GPU parity; export/cancel cleanup; device-clock interpolation |
| Inference boundary tests | 4 tests: bounded finite tensors, preflight cancellation, forged SAM memory/prompts, source-coordinate resampling and incompatible RVM state geometry |
| Commands/reference/automation | 8 tests, including 4 already counted in core: atomic groups, monotonic undo/redo, owner/stale/overflow checks; immutable native fixture replay; real MCP process workflows under both protocol generations; outside edits/deletion; competing servers; source inventory/error/no-overwrite behavior |
| Native workspace/bank | 16 tests: incremental deep catalogs and cancellation, palette parsing, settings concurrency/corruption, exact asset round trips/conflicts, stale recovery/save ownership, inactive recovery, bank draft merging and native input/focus regressions |
| Vendored native SDK | 9 focused Wayland redraw and clipboard/modifier tests passed |
| Native kill/recover/reopen | 100/100 trials passed with 4,000 catalog documents; 50 untitled and 50 saved originals; active/inactive acknowledged checkpoints retained, independent recovery identities and unchanged source/checkpoint hashes |
| Native input/publication | 250 inputs: p50 11.768 ms, p95 25.086 ms, maximum 65.256 ms; 200 UI checkpoint commits: p95 1.059 ms, maximum 4.585 ms; peak RSS/HWM 103,248 KiB |
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

## Still to implement

Checkpoint pruning, timeline/composition schema and expanded commands, production ingest/playback,
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
qualified. R2–R11 remain roadmap work. No enterprise or GTM completion
is declared.
