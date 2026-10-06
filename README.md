# EditBay

A native Rust production suite for Omarchy: editing, motion graphics, VFX, color,
sound, and connected client/team workflows. The goal is to let professionals move
their existing production work from macOS/Windows to Omarchy.

The Rust implementation provides a **native local workspace**, **project/recovery
CLI commands and scoped native MCP automation**, plus
**native codec, GPU, sound-clock, export and video-inference feasibility tools**.
The workspace includes welcome, project tabs, checked saves, automatic recovery,
local settings and shared brand banks. Timeline/media authoring and complete-job
release gates remain roadmap work. The local R1 workspace gates pass; see the
[native workspace evidence](docs/evidence/r1-workspace/README.md).
The application and production workers use Rust with native dependencies.

## Roadmap and decisions

- [Complete implementation roadmap](docs/ROADMAP.md): stable issue IDs, dependencies,
  all 18 gap areas, production AI, and complete-job acceptance gates.
- [Implementation status and validation](docs/IMPLEMENTATION_STATUS.md): what works
  in the Rust rewrite today and what remains unimplemented.
- [Rust architecture](docs/ARCHITECTURE.md): native UI, typed compositions, GPU/color,
  audio, jobs, recovery, inference, and cloud boundaries.
- [Omadesign reference](docs/OMADESIGN_REFERENCE.md): actual 0.6.3 source locations
  for welcome, recovery, projects, assets, cloud, and native ML shipping.
- [AI models](docs/AI_MODELS.md): Meta-first candidates, capabilities, licenses,
  portrait/video matting, offline edge refinement, optional local vision review,
  native runtime feasibility, tracking, and rigging.
- [Migration and adoption](docs/MIGRATION_AND_GTM.md): existing-project migration,
  client workflows, cross-app assets, user acquisition, and earned retention.
- [Release criteria](docs/RELEASE_CRITERIA.md): measurable enterprise/GTM gates.
- [Native engine evidence](docs/evidence/r0-engine/README.md): actual ARM64
  decode, FP16/FP32 composition, device-clock playback, export and cancellation.
- [Native inference evidence](docs/evidence/r0-inference/README.md): pinned SAM 2.1
  video/RVM artifacts, recurrent resume, active cancellation and independent references.
- [Native automation](docs/AUTOMATION.md): scoped tools, typed commands, revision
  ownership, atomic undo groups and future worker/inspection requirements.
- [Reference jobs and fixtures](docs/evidence/r0-reference/README.md): actual local
  source inventory, fixed native/interchange inputs and automation validation.
- [Your local workspace](docs/LOCAL_WORKSPACE.md): native creation, saves, recovery,
  settings and shared assets.
- [Typed document model](docs/DOCUMENT_MODEL.md): schema migration, sources,
  retiming, compositions, animation, commands and frame-plan inspection.
- [Shared brand banks](docs/SHARED_BRANDS.md): compatible manifests, exact asset
  copies, version ownership and collision rules.

## Run the Rust foundation

Build on the local machine with Rust/Cargo:

```sh
mkdir -p artifacts/tmp
export TMPDIR="$PWD/artifacts/tmp"
cargo build --workspace --locked
cargo test --workspace --locked
./bin/editbay
./bin/editbay /path/to/client/cut.editbay
./bin/editbay --help
./bin/editbay new /path/to/client/cut.editbay "Client spot"
./bin/editbay info /path/to/client/cut.editbay
./bin/editbay rename /path/to/client/cut.editbay "Client spot revised"
./bin/editbay apply /path/to/client/cut.editbay /path/to/commands.json
./bin/editbay frame-plan /path/to/client/cut.editbay COMPOSITION_UUID 24
./bin/editbay migrate /path/to/legacy.editbay /path/to/new-format.editbay
./bin/editbay probe-media /path/to/camera.mp4
./bin/editbay ingest /path/to/client/cut.editbay /path/to/camera.mp4 0,1
./bin/editbay decode-frame /path/to/camera.mp4 1 SOURCE_TICK
./bin/editbay checkpoint /path/to/client/cut.editbay /path/to/recovery
./bin/editbay recoveries /path/to/recovery
./bin/editbay recover /path/to/checkpoint /path/to/Recovered.editbay
./bin/editbay-mcp /path/to/client/cut.editbay /path/to/recovery
```

The native executable is `target/debug/editbay-studio`; the CLI is
`target/debug/editbay`. The source launcher chooses the native app for no arguments
or project paths, and the CLI for the listed commands. It builds/runs Rust with
the lockfile. Projects use a fresh validated schema and stable
UUID identity. Frame rates stay rational. Saves use synchronized atomic writes
with writer locks and stale-edit rejection for edits. New files are published
without replacing an existing destination. Checkpoints are immutable and integrity
checked; recovery creates an independent copy and refuses to replace the original
or an existing destination. Invalid checkpoints stay visible as errors alongside
older valid work.

`cargo run --release --locked -p editbay-lab -- --help` lists native feasibility
commands. FFmpeg development libraries and ALSA are required for those crates;
the codec tests also use the FFmpeg executable to make synthetic fixtures.
Rust 1.95 or newer matches the pinned Omadesign native stack requirements.

`editbay-lab sound-blocks SOURCE` qualifies bounded original-channel sound
against independent native PCM and declared worker timing/memory budgets.
See [sound block contracts](docs/SOUND_BLOCKS.md). Native streaming playback
and shared delivery are subsequent R2 dependencies.

`editbay-lab inventory MEDIA_DIRECTORY NEW_JSON_REPORT` recursively inventories
local source hashes and first decoded video pictures, reports incomplete/error
states and refuses to overwrite an existing report. Native MCP currently exposes
document inspection, project rename groups, undo/redo, checkpoint and recovery
inspection. It uses the core command path and acknowledges edits after durable
save. Its startup paths grant scope; RPC calls cannot choose arbitrary files.

Typed sources/compositions/animation and frame-plan inspection now share the core
command, undo, save and recovery path. Existing schema 1 projects/checkpoints
migrate in memory; their source bytes remain intact until explicit save. Native
media import selects actual streams, indexes picture timestamps and preserves
original sound channels through undo/save/recovery; see
[native ingest](docs/MEDIA_INGEST.md). Native timeline gestures, evaluated
picture/sound output, full media delivery, cloud and artist inference workflows
remain roadmap work. The lab's picture-only exporter and 30-second
sound probe have measured limits; see the evidence before choosing a workload.

`EvaluationSnapshot` retains immutable compiled dependencies for repeated exact
fractional/reverse frame planning. The published integer inspection format is
preserved. `editbay-lab evaluation SOURCE ITERATIONS [SYNTHETIC_INDEX_PICTURES]`
measures actual indexed source planning and optional, explicitly synthetic index
stress. See [temporal evaluation](docs/TEMPORAL_EVALUATION.md). This does not yet
provide rendered composition playback or export.

`editbay-lab picture-cache SOURCE` qualifies the bounded, source-owned native
decoded-picture provider against actual sequential pixels. Cached and externally
held outputs stay charged through eviction, cleanup and document rebinding. See
[decoded pictures](docs/PICTURE_CACHE.md) for ownership, memory and remaining
native playback integration.

`editbay-lab render-graph SOURCE` qualifies the shared typed SDR picture renderer
against real native source pixels at FP16/FP32. Working textures stay resident;
display/output conversion and float readback are explicit boundaries. See
[GPU pictures](docs/GPU_PICTURES.md) for supported operations, color, ownership,
budgets and remaining native presentation/process/audio integration.

Keep project builds, worktrees, temporary files and private evidence under this
checkout. `/target/` and `/artifacts/` are ignored. Set `TMPDIR` as above for local
tests so their temporary fixtures also stay here. The older committed evidence
retains its original artifact paths; those historical paths are not working
storage instructions or promises that private binaries remain available.

`editbay-lab picture-worker SOURCE [WORKER_BINARY]` qualifies the retained native
codec process and sealed, bounded RGBA handoff. `render-graph-worker` runs the
same typed GPU graph through that provider. The packaged app/CLI both host this
Rust endpoint. See [isolated pictures](docs/WORKER_PICTURES.md) for source/receipt
ownership, consumer pins, cancellation/retry and qualification limits. Native
surface presentation now uses the same graph on the window's own device. Create
a sequence from an imported video, step or scrub its exact frame numbers, then
save or recover the editable document. Cancel and Retry supervise the actual
codec job. See [native preview](docs/NATIVE_PREVIEW.md) for ownership, timing,
color and qualification limits. Audio-clock scheduling and complete delivery
remain open.

`editbay-lab pcm-worker SOURCE [WORKER_BINARY]` checks bounded original-channel
sound transport through a packaged app/CLI codec child. `sound-blocks-worker`
uses that route in the same nested gain/mix renderer as `sound-blocks`.
See [isolated sound](docs/WORKER_SOUND.md) for ownership, budgets, cancellation
and the remaining streaming playback and delivery gates.
