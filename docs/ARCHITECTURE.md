# Rust architecture

Decision date: 2026-10-03. This defines the target architecture. Only crates
present in the workspace are implemented; proposed modules are not stubs.

## Boundaries

The native product, command layer, job supervision, recovery, and production
workers are Rust. FFmpeg/libav, OpenColorIO or equivalent audited color libraries,
ONNX Runtime/libtorch or validated native inference, and optional native SDKs
can sit behind narrow interfaces. No shipped Python/Conda/PyTorch-Python backend.
The website/cloud backend can reuse Omadesign's established web/service stack;
the Rust requirement applies to the native product and production processing.

Use eframe/egui for continuity with Omadesign and wgpu for GPU rendering. Match
the deployed Omadesign versions and applicable Wayland fixes after a compatibility
probe. Do not couple EditBay to a relative path into an Omadesign worktree or copy
its entire application. Extract small reusable crates with independent tests.

| Module | Responsibility | Boundary |
| --- | --- | --- |
| `editbay-core` | Typed document, rational time, validation, persistence/recovery foundation | No GUI/media/network dependency |
| `editbay-cli` | Functional project/recovery commands now; later automation/export | Uses the same core as desktop |
| `editbay-app` | Welcome, workspaces, input, command/search, job/status UI | UI never runs blocking codec/model/cloud work |
| `editbay-media` | Ingest, metadata, stream mapping, decode, proxies | Native codec adapters with bounded memory |
| `editbay-render` | Typed temporal image graph, color, masks, float GPU processing | Shared preview/export evaluation |
| `editbay-audio` | Audio clock, sample-accurate graph, routing, devices, automation | Real-time callback excludes allocation/blocking work |
| `editbay-jobs` | Scheduling, cancellation, resource budgets, worker supervision | Revision- and source-owned results |
| `editbay-ai` | Model registry, native runtimes, matte/track/rig jobs | Editable persistent results; no mandatory cloud |
| `editbay-interchange` | FCPXML/XML/OTIO/AAF/graphics codecs and conversion reports | Read-only sources; new destination files |
| `editbay-cloud` | Identity/device auth, transfers, versions, review, roles | Explicit sync; local authority remains intact |
| `editbay-automation` | Native MCP, inspection, commands, preview and job access | Same revision/validation/undo path as the UI; no bypass writes |
| Shared Oma components | Theme/fonts, catalogs, brand/assets, format codecs, recovery patterns | Versioned APIs, independent app identities |

The workspace now includes the native `editbay-app` local workspace, core/CLI,
and narrow media, render and audio
feasibility crates, native `editbay-ai` video/matting prototypes, optional independent
reference kernels, the Rust `editbay-lab` tool and scoped `editbay-automation` MCP.
These prototypes do not establish
the full production contracts in the table. Remaining modules describe future
ownership. Split crates when real dependency/ownership boundaries justify it.

## Document and composition model

- Stable project, sequence, composition, source, clip, node, and track identities.
- Rational frame rate; integer frame/sample boundaries; explicit conversion among
  source, clip-local, sequence, composition, and wall-clock time.
- A composition is reusable by an edit, motion scene, color operation, or VFX
  graph. Nesting and retiming do not flatten animation, mattes, or dependencies.
- Nodes declare image/mask/geometry/audio/data sockets and temporal dependencies.
  Multiple-input composition and graph validation are first-class requirements.
- Effects, scalar/vector/path channels, interpolation, rig constraints, and
  expressions are structured document data. No arbitrary Python evaluation.
- Asset references record paths, identities, provenance, hashes, media metadata,
  font/model dependencies, and any migration conversion notes.
- Local authoring files, disposable caches, immutable recovery checkpoints,
  portable asset packages, and cloud revisions have distinct ownership.

Schema 2 implements typed assets/streams, tracks/clips, nested compositions, timed
image/mask/geometry/audio/data nodes, channels, exact source mapping and separate
color settings. Typed frame plans expose evaluated parameters and source requests;
the shared SDR renderer consumes their picture subset. Full audio/mask/HDR/native
playback remains open. See [document model](DOCUMENT_MODEL.md).
`EvaluationSnapshot` now compiles immutable node order, shared static operations
and source interpretation hashes once. Exact fractional/reverse preparation and
integer inspection share that evaluator. Working image/mask keys include their
dimensions, source/mask bytes, dependencies, gamut and float precision; document
version separately controls publication. Audio keys describe a point, not a block.
The complete GPU/audio engines remain open; see
[temporal evaluation](TEMPORAL_EVALUATION.md).
The native decoded-picture provider retains exact indexed source requests,
immutable raw pixels, decoder cursors and declared cache/live-output/handle limits.
Consumer-held pixels remain charged after eviction. Rebinding invalidates prior
receipts and preserves matching content under a fresh cancellation token. Native
process ownership now uses the [isolated retained codec route](WORKER_PICTURES.md)
over sealed binary planes. The native viewer uses that provider on the UI's actual
GPU device; see [native preview](NATIVE_PREVIEW.md) and
[decoded pictures](PICTURE_CACHE.md).
The shared SDR GPU renderer now consumes exact prepared source/nested picture,
solid, affine, over and scalar-opacity graphs. Linear premultiplied FP16/FP32
textures stay resident; display/output conversion and readback are explicit.
Cache/live texture limits retain consumer and submitted-work charges; private
worker/version/generation receipts reject stale publication. Mask/HDR/full color
and the audio interval engine remain
open. See [GPU picture contracts](GPU_PICTURES.md) and their measured evidence.
The native display draw checks exact device identity, applies the declared SDR
display boundary and retains charged resources until its actual GPU submission
completes. It introduces no normal preview readback. A bounded preview actor
coalesces requests, rejects obsolete results and cancels underlying work after
session/revision/view changes. Source-sequence authoring validates off-thread and
commits one normal undo group under captured ownership. See
[native preview contracts](NATIVE_PREVIEW.md).
Schema 1 migrates after integrity verification without writing its source; unknown
schemas fail. Format-copy migration uses a separate destination.

Selected native media ingest now fills these source records through ordinary
asset/source commands. The packaged Rust codec child retains a read-only source
descriptor and full checksum, indexes actual picture timestamps, and preserves
original sound rate/channel order. Off-thread preparation and final source
verification precede session/generation/revision-owned UI publication; crash and
cancellation leave the document intact. Bounded protocol/decoder/process limits
and current format/precision limits are defined in [media ingest](MEDIA_INGEST.md).

## Commands and concurrency

UI, keyboard, CLI, MCP, and automation dispatch validated commands. A command
produces a revision and undo group, identifies render/cache invalidation, and
marks recovery work dirty. Read-only inspection does not mutate the document.

The current core `DocumentEditor` owns atomic typed document groups, monotonic
revisions and bounded undo/redo. CLI apply, rename and MCP share it. Immutable shared
snapshots keep graph copying out of frame/save/recovery handoffs. The MCP service
saves a candidate through the existing checked writer before replacing its
session state/history. Only implemented tools are advertised. See
[automation contract](AUTOMATION.md) for actual scope and subsequent job/source/
preview requirements. The native UI uses this same editor for rename and history;
checked saves and separate recovery copies retain the foundation contracts.

Expose real document/render snapshots and job state through a native MCP adapter.
List capabilities accurately; require expected revisions for mutations; surface
validation failures and conversion losses. Keep filesystem/network access within
the artist's granted scope. Agent-authored titles, edits, mattes, and animation
are ordinary editable document data with provenance and undo.

Workers receive immutable revision snapshots or explicitly owned resources.
Completion includes job ID, document ID, revision, and source/model fingerprints.
The receiver rejects stale work after edit, close, save-as, tab replacement, or
cache-key changes. Cancellation must reach the underlying work, not just hide UI.
Media decode, encode, inference, file dialogs, scanning, and network transfers
never block egui's frame/input path. Native library/plugin crashes are isolated
in worker processes where the risk warrants it.

The audio clock schedules playback. Decode/render falls behind through a defined
drop/degrade policy instead of slowing the sound clock. Source rational rates,
drop-frame timecode display, and VFR timestamp mapping have dedicated fixtures.

## Color and GPU

Represent input transfer/gamut/range, working space, output transform, and display
transform separately. Specify precision and premultiplication at every boundary.
Use linear float composition with a measured FP16/FP32 policy; formats may have
different source/output precision. GPU capability checks select an honest fallback.
Avoid CPU readback during normal preview; readback/export is an explicit boundary.

Preview and export share the graph, time mapping, animation, masks, alpha, and
color configuration. Preview quality may reduce resolution/evaluation cost while
preserving semantics. Cache invalidation includes color and temporal dependencies.

Prototype decode-to-texture interoperability and multi-GPU/backend behavior before
committing to a universal zero-copy claim. FFmpeg provides native media plumbing;
the typed document and composition graph own editing/render semantics.

## Offline matte analysis

Ingest/AI adapters declare whether outputs are segmentation scores, class maps,
opacity alpha, foreground RGB, tracks or relative depth. Keep those types
distinct in the render graph. Model resize/range/color preprocessing maps back
to source coordinates; the compositor retains the original image and explicit
premultiplication policy.

A final matte job processes source frames rather than viewer frames. Stateful
video models own recurrent resources per shot, run in source order, reset at
cuts, and checkpoint enough state for validated resume/replay. Independent shots
can run concurrently within resource limits. Store output masks and refined
float alpha as source-time assets; retiming/nesting maps to those assets.

Source-resolution refinement and temporal quality checks precede publication.
An optional local vision controller proposes bounded region/frame parameter
tracks through revision-aware commands, with temporal regularization, undo and
artist overrides. It has a finite evaluation budget; its improvement over the
matting/refinement baseline must be measured before production enablement.

Only committed matte assets enter preview/export and portable handoff. A worker
cannot replace newer manual corrections. Cache invalidation includes model,
state/configuration, spatial correction and affected temporal dependencies.
Recipients use committed results without rerunning the analysis models.
See [model/workflow contract](AI_MODELS.md#offline-matte-finishing-and-optional-local-vision-review).

## Recovery contract

Adapt Omadesign's revision ownership and worker preparation pattern:

1. Edits mark a document/revision dirty. Idle debounce and bounded maximum delay
   schedule recovery for active and inactive documents without interrupting gestures.
2. A worker prepares a durable checkpoint. It reports disk-full/permission errors.
3. A main-thread ownership check commits only a still-relevant result. Manual
   saves, replacement, and close cannot resurrect an obsolete checkpoint.
4. Checkpoints contain stable identity, schema, revision, time, source path, and
   an integrity check. A broken newest checkpoint leaves older valid ones visible.
5. Recovery opens a separate copy. Recovered work appears in the welcome catalog;
   the saved original survives until an explicit user save choice.

Writes use a unique temporary file in the destination directory, complete bytes,
file synchronization, and parent-directory synchronization on supported native
filesystems. Existing-file saves publish by atomic rename; new files/checkpoints
publish through a hard link that fails if the destination already exists. Newly
created parent directories are synchronized too. Persistent sidecar locks
coordinate writers; checked edits compare the loaded document under the lock to
reject stale updates. Account for filesystem-specific durability rather than
promise arbitrary network filesystem behavior.

The Rust foundation and native workspace implement validated save/load and a
worker-prepared checkpoint with a short ownership-checked publication. Every dirty
tab participates in idle/maximum-delay recovery. Native history/preview and
separate-copy recovery are implemented. History pruning and native timeline gestures
remain later work. EditBay's local settings and bank metadata have their
own versions and optimistic writer ownership; see [workspace](LOCAL_WORKSPACE.md)
and [shared banks](SHARED_BRANDS.md).

## Cloud and large media

Reuse Omadesign's proven role/device-link/version concepts with an explicit
EditBay adapter. Project metadata can share account/client/team context only
after the user connects it. A production video service needs chunked/resumable
transfers, range playback, proxy derivatives, checksums, quotas, and an explicit
storage/egress policy. Existing still-design upload limits do not fit video.

Do not edit the running Omadesign production schema during the rewrite. Define
and test an additive integration in an isolated development environment first.
Client review and explicit source version exchange are the first collaboration
scope; live concurrent authoring needs a separate conflict/ownership protocol.

## Proof before expansion

First prove representative decode/playback/export and native segmentation on
real hardware, then integrate them into one complete project. Record numerical
budgets and actual measurements for input latency, frame time, seek latency,
audio drift, memory, job cancellation, and exports. Compile success does not
prove the artist's workflow or a GPU/model route. No fake frames, fake progress,
placeholder exporters, or enabled-but-unimplemented controls.
