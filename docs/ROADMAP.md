# EditBay: the Omarchy production suite

Updated: 2026-10-03. This is the implementation roadmap for a fresh Rust product.
Milestones describe required behavior, not capabilities already shipped.

## Product mandate

Let an editor, motion designer, VFX artist, colorist, or audio professional move
their paying work from macOS/Windows to Omarchy. One portable project connects
editing, motion, compositing, color, audio, assets, client review, and delivery.
Preserve the full-suite goal while releasing complete workflows in sequence.

The native application and production workers are Rust. Native dependencies such
as FFmpeg, color-management libraries, and inference runtimes are permitted.
There is no production Python interpreter, Python editor backend, Conda setup,
or Python inference worker. Research model examples do not determine the shipped
runtime. A rewrite alone does not establish professional performance or quality.

Use [architecture](ARCHITECTURE.md), [Omadesign reference](OMADESIGN_REFERENCE.md),
[AI integration](AI_MODELS.md), and [migration/adoption](MIGRATION_AND_GTM.md)
alongside this roadmap. Stable issue IDs below support later GitHub publication.
See [implementation status](IMPLEMENTATION_STATUS.md) for the first Rust slice
and its validation; milestone completion is broader than that initial slice.

## Status and execution rules

- The Cargo workspace is the application implementation and source of truth.
- Rust project persistence/recovery is the initial implementation slice. A Rust
  desktop editor, media engine, and cloud integration remain roadmap work.
- Each issue needs implementation, representative fixtures, user-path validation,
  documentation, and measured limitations before it is complete.
- Share Omadesign components through small, versioned interfaces after proving
  them in both applications. Do not import its entire application as a dependency.
- Compile, test, package, and validate native Rust locally. Hosted site CI is
  separate from desktop validation. Never label queued native CI as a pass.
- Every release names which complete workflows it supports. No feature badges
  inferred from a library dependency or a model README.

Current R0 progress: EB-001–003 foundation is preserved; EB-004 has measured native
ARM64 decode, sound-clock, float GPU and isolated export prototypes. See
[engine evidence](evidence/r0-engine/README.md). EB-005 now has native SAM 2.1 video
and RVM prototypes, active cancellation, exact recurrent resume and independent
kernel comparisons on ARM64; see [inference evidence](evidence/r0-inference/README.md).
Production artist/model-pack qualification remains R4/R11 work. EB-006 now has an
actual source/job inventory and fixed native/migration inputs; EB-007 has the
scoped native MCP contract and implemented core command/undo tools. See
[reference evidence](evidence/r0-reference/README.md) and [automation](AUTOMATION.md).
R0's foundation and documented ARM64 feasibility gates pass; R1 is next.
Quantitative enterprise/GTM budgets are set in [release criteria](RELEASE_CRITERIA.md).

## Milestone map

| Milestone | User outcome | Dependencies |
| --- | --- | --- |
| R0 | Rust project foundation and native feasibility | None |
| R1 | Omadesign-quality welcome, projects, saves, and recovery | R0 |
| R2 | Responsive, color-aware media and composition engine | R0; work alongside R1 |
| R3 | Complete commercial/social/interview editing workflow | R1, R2 |
| R4 | Editable auto-roto, object tracking, and practical shot repair | R2; model feasibility starts at R0 |
| R5 | Motion design and general node/layer compositing | R2, R3; coordinate with R4 |
| R6 | Multicam, transcript editing, captions, and retiming | R3 |
| R7 | Professional color finishing | R2, R3, R4 |
| R8 | Professional sound postproduction | R2, R3 |
| R9 | Client/team cloud workflow and Omadesign asset exchange | R1, R3; backend/media design starts at R0 |
| R10 | Advanced 3D, rigging, retargeting, and production integration | R4, R5 |
| R11 | Specialist delivery, extensions, hardware, and broad launch | Relevant earlier milestones |

These are dependency gates, not a promise that one person can work all streams
simultaneously. Dates follow resource decisions and successful engine/model
prototypes; the full-suite target is a substantial multi-year program at solo
scale. Early releases serve real jobs without shrinking the eventual mandate.

## R0 — restart in Rust

- **EB-001:** Keep a Rust-only application workspace with accurate entry points,
  current implementation documentation, and focused agent guidance. Exclude
  retired implementations and their fixtures, site assets, and documentation.
- **EB-002:** Establish Cargo workspace, dependency lockfile, structured errors,
  rational frame rates/time, stable document identity, and versioned project
  validation. Model time explicitly; do not use floating-point seconds as edit
  boundaries or mix source, sequence, and composition time.
- **EB-003:** Implement durable atomic project writes and immutable, checksummed
  recovery checkpoints, with schema rejection and recovery to a separate file.
- **EB-004:** Prototype FFmpeg decode, audio-clock playback, wgpu composition,
  half/full-float color transforms, and background export on actual machines.
  Record feasibility, memory, copies, frame timing, and backend compatibility.
- **EB-005:** Prototype native SAM-family and human-matting inference with approved artifacts;
  demonstrate preprocessing, stateful video propagation, cancellation, hardware
  support, and numerical/output agreement with reference inference.
- **EB-006:** Establish reference projects and evidence storage. Inventory real
  client production jobs and migration fixtures; conversion always writes a
  separate destination and includes an unsupported-feature report.
- **EB-007:** Specify the native automation/MCP contract over the same document
  commands as the UI: capabilities, inspection, revisions, undo groups, source
  permissions, job status/cancellation, and actual preview/export inspection.
  Omarchy's chosen agents author editable work through this contract; production
  inference models and the artist's assistant provider remain independent choices.

Gate: project creation/save/load/checkpoint/recovery works in Rust with corruption,
interruption, and concurrency tests. Engine and model feasibility are measured;
this milestone does not claim a finished editor.

## R1 — the familiar Omadesign front door

- **EB-010:** eframe/egui desktop shell using the Omarchy theme, typography,
  spacing, icons, command/search patterns, and quiet hierarchy of Omadesign.
  Adopt applicable Wayland input/clipboard/IME fixes with focused validation.
- **EB-011:** Three-column welcome: Your Work, creation/help actions, Projects.
  Natural-aspect masonry; newest work first; Recovered as a first-class view;
  explicit multi-open; search/filter; useful empty states and cover frames.
- **EB-012:** Responsive filesystem catalog with incremental results, cancellation,
  nested project navigation, honest incomplete/error states, and hidden/trash/
  symlink exclusions. `.omabrand` directories are shared project context.
- **EB-013:** Idle/debounced background recovery for every dirty document, including
  untitled/inactive documents. Reject obsolete worker results after save, close,
  or document replacement; preserve the newest recoverable revision.
- **EB-014:** Dirty-state close/new/open guards, checkpoint history, recovery
  previews, backup export, save-as/copy behavior, schema migrations, and visible
  disk-full/permission errors. No silent failure or overwrite of the saved original.
- **EB-015:** Persist startup/workspace/shortcut preferences atomically. Restore
  tabs and local work independently of cloud login. Native file dialogs run off
  the UI thread and cannot apply a late result to a different document.
- **EB-016:** Shared brand bank: fonts, palettes, logos, artwork, title packages,
  LUTs, and client/project metadata. Define collision/version rules for shared
  Omadesign/EditBay folders; do not replace an existing `.omabrand` schema casually.

Gate: create, find, save, kill, recover, reopen, and navigate several actual client
projects without blocking input or losing work. Test small windows and differing
Omarchy palettes. Recovery tests do not substitute for native visual inspection.

## R2 — the production engine

- **EB-020:** Typed media sources, tracks, nested compositions, timed nodes,
  masks, animation channels, asset references, and revision-aware commands.
  UI, CLI, MCP, undo, recovery, and rendering use one validated command path.
- **EB-021:** FFmpeg ingest/decode and stream selection; VFR interpretation;
  source timecode/reel metadata; image sequences; multi-channel audio. Add camera
  RAW/video SDKs only with explicit supported formats and tested controls.
- **EB-022:** Hardware decode/encode adapters and GPU texture handoff; CPU fallback;
  bounded caches; background thumbnails, waveforms, proxies, and cache cleanup.
- **EB-023:** Audio-clock synchronization, frame scheduling/drop policy, scrubbing,
  seeking, reverse/shuttle, and long-duration drift verification.
- **EB-024:** Typed GPU graph with multiple image/mask/data inputs, float working
  images, explicit straight/premultiplied alpha, input/working/display/output
  transforms, and reusable compositions. Preview and export share evaluation.
- **EB-025:** Background jobs with progress, cancellation, retry, resource budgets,
  source/revision ownership, and crash containment. Export cannot freeze the UI.
- **EB-026:** Cache keys include sources, edit revision, color configuration,
  graph dependencies, and model version. A correction invalidates only affected
  work; previous results cannot overwrite a newer edit.

Gate: documented representative 1080p/4K playback, seek, cache, export, and long
audio-sync measurements across Intel/AMD/NVIDIA and ARM64. Set numerical budgets
before measurement; publish actual results and degradation modes per workload.

## R3 — finish a paying edit

- **EB-030:** Source/record monitors, in/out, three-point edits, track targeting,
  sync locks, linked sound, precision trim/roll/slip/slide, snapping, ripple,
  overwrite/insert, and dependable keyboard workflows.
- **EB-031:** Nested/compound sequences, reusable selects, alternate cuts,
  timeline comparison, markers, adjustment layers, and predictable undo grouping.
- **EB-032:** Background ingest, searchable bins, ratings, selects, custom metadata,
  checksum copy, bulk relink, consolidate/trim/archive, and proxy/original exchange.
- **EB-033:** Baseline editable titles, transitions, transforms, masks, sound mix,
  color controls, range export, and a cancellable delivery queue.
- **EB-034:** FCPXML and Premiere XML migration with explicit conversion notes;
  OTIO interchange; honest supported subsets and test fixture comparisons.
- **EB-035:** Shortcut presets and workspace layouts for FCP/Premiere/Resolve
  habits. Connected/magnetic-style editing and track-based editing share sync
  invariants; validate them with users rather than reproduce UI labels alone.

Gate: deliver real commercial, social, and interview projects, including revision,
archive/reopen, and a migration from each supported interchange source.

## R4 — AI roto, tracking, and shot repair early

- **EB-040:** Model manager: verified hashes, provenance, licenses, approved download
  sources, storage/runtime estimates, backend selection, and offline operation.
- **EB-041:** SAM 3.1/SAM 2.1 candidates for click/box/text subject selection and
  forward/backward mask propagation. Non-destructive matte tracks survive save,
  copy, trim, retime, composition nesting, and export.
- **EB-042:** Correction keyframes, multiple subjects, occlusion/reappearance,
  confidence/visibility, hair/edge refinement, feather/expand, and motion-blurred
  edge treatment. Compare with representative production footage, not only demos.
- **EB-043:** Point/planar/object tracking with editable tracks; attach text/effects,
  stabilize, and corner-pin screens. CoTracker is a research reference until its
  noncommercial terms are resolved; ship a cleared native alternative if needed.
- **EB-044:** Paint/clone, tracked cleanup patches, spill suppression, refined keying,
  lens correction, and optional inpainting adapters with separately verified terms.
- **EB-045:** AI work is background/cancellable/resumable. Cache masks/tracks as
  project assets so recipients can render without rerunning the model. Keep
  manual authoring and correction available when inference cannot run locally.
- **EB-046:** Evaluate the supplied SINet, MediaPipe/Meet, Selfie Segmentation,
  Selfie Multiclass, PP-HumanSeg, RVM and TCMonoDepth candidates by role. Pin exact
  artifact provenance/terms; distinguish segmentation, opacity and depth; preserve
  useful soft outputs. RVM's official ONNX path is a native feasibility candidate.
- **EB-047:** Implement a final offline matte job over every source frame, with
  source-resolution edge/matting refinement, motion-aware neighboring-frame checks,
  shot resets, recurrent-state resume/replay, and editable cached float results.
  Final precision and correction can exceed the interactive preview budget.
- **EB-048:** Evaluate an optional local vision correction controller. Feed
  boundary crops, original/composite/alpha views and neighboring frames; propose
  bounded region/frame parameter tracks through normal revision/undo commands.
  Regularize parameters across a shot, retain artist overrides, cap retries, and
  benchmark incremental value over a dedicated matting/refinement pass.
- **EB-049:** Qualify raw-mask, video-matting, refined/temporal and vision-assisted
  routes on identical source footage. Measure alpha/boundary quality, flicker,
  leakage/halos, correction time, latency and memory. Preview/export use identical
  committed matte assets; handoff includes unresolved-frame review and portable
  results that need no further inference.

Gate: editable isolation and screen replacement through occlusion, cuts, fast
motion, and changed timing. Offline matte quality and any local vision improvement
are measured; stateful resume matches uninterrupted processing. Export matches
the viewer; no Python production path. See [offline workflow](AI_MODELS.md#offline-matte-finishing-and-optional-local-vision-review).

## R5 — After Effects/Fusion-class authoring

- **EB-050:** Temporal/spatial graph editor, Bézier handles, motion paths,
  anchor points, nulls, constraints, parenting, expressions, and reusable rigs.
- **EB-051:** Text shaping/layout, per-character/word selectors, text on paths,
  kinetic typography, and portable font dependencies.
- **EB-052:** Editable vector shapes/paths, strokes/fills/gradients, trim paths,
  repeaters, deformation, masks, motion blur, and responsive templates.
- **EB-053:** Layer and node authoring over the same composition model; branching
  merges, track mattes, blend/channel operations, node groups/macros, and debug views.
- **EB-054:** Live/reloadable Omadesign artwork and motion assets; PSD/PSB, SVG,
  OpenRaster, Lottie, and image sequences using shared codecs where proven.
  Report preserved/editable/baked/missing features for every interchange route.
- **EB-055:** Publish parameterized titles/effects/transitions; instantiate them
  in edits, revise their source, version them, and share complete packages.

Gate: an editable brand film, kinetic typography, tracked graphics, and a multi-
source VFX composite can be revised without manual render/export round trips.

## R6 — interviews, events, documentaries, and creator speed

- **EB-060:** Multicam construction; audio/timecode sync; angle viewer; live angle
  cuts; separate picture/sound switching; per-angle correction and replacement.
- **EB-061:** Transcription, speaker labels, transcript search/editing, and local
  inference candidates with measured accuracy and explicit language coverage.
- **EB-062:** Caption tracks, SRT/VTT/TTML, multiple languages, editable styles,
  subtitle deliverables, and revision-safe caption timing.
- **EB-063:** Retime curves, speed ramps, reverse/freeze, interpolation/optical flow,
  and pitch-aware sound policy. Verify source-frame mapping throughout interchange.
- **EB-064:** Aspect-ratio variants, safe areas, automatic reframing with manual
  correction, reusable client delivery profiles, and multiple output versions.

Gate: finish a multicam event and a transcript-driven interview with captions and
format variants; preserve sync and editability through revisions.

## R7 — color and finishing

- **EB-070:** ACES/OCIO-capable management; log/RAW transforms; exposure/white balance;
  SDR/HDR output; tone mapping; calibrated viewing and correct output metadata.
- **EB-071:** Waveform/parade/vectorscope/histogram/gamut scopes; curves, wheels,
  qualifiers, tracked windows, parallel/serial grades, and selective corrections.
- **EB-072:** Grade groups, shot matching, still gallery, grade versions, LUT
  management, temporal denoise, and reconform from revised offline edits.
- **EB-073:** High-precision still/sequence/video exports and reference comparisons
  for range, chroma, alpha, bit depth, display transforms, and round trips.

Gate: independently verify SDR/HDR finishing and a grade/reconform job against
reference transforms, scopes, files, and supported monitoring hardware.

## R8 — sound postproduction

- **EB-080:** Sample-accurate audio model; channel mapping; buses/sends/returns;
  inserts/sidechains; automation modes; latency compensation and audio-clock ownership.
- **EB-081:** PipeWire/device management, recording/voiceover/ADR, punch-in,
  take handling, monitoring, and control-surface integration.
- **EB-082:** Dialogue restoration, EQ/dynamics, loudness/true-peak measurement,
  music ducking, and licensed source-separation inference, including SAM Audio evaluation.
- **EB-083:** Multichannel/surround layouts, stems, bounce/freeze, AAF/audio handoff,
  and verified broadcaster/client loudness/channel requirements.

Gate: edit and mix an interview, dialogue scene, and multichannel project; deliver
validated stems and mixes and exchange them with an external audio workflow.

## R9 — client/team work and ecosystem retention

- **EB-090:** Adapt Omadesign identity/device-link, owner/editor/reviewer semantics,
  private versioned assets, explicit push/pull, and revocation-aware downloads.
- **EB-091:** Design resumable/chunked large-media transfer and storage/egress limits.
  Omadesign's current 100 MB source/asset limits are unsuitable for production video.
- **EB-092:** Browser video review with frame/timecode-accurate annotations,
  persistent links, version comparison, approvals, and mobile access.
- **EB-093:** Offline-first project ownership, shared libraries, explicit conflict
  resolution/leases, immutable versions, and restore to a separate local copy.
- **EB-094:** Shared Omadesign/EditBay brand context and cross-app editable assets,
  templates, fonts, palettes, client projects, and team libraries.
- **EB-095:** Public showcase/templates, client invitation loops, opt-in community
  adoption, and exportable project/team data. Collaborative live authoring requires
  a separate synchronization/ownership design; review does not imply live editing.

Gate: two accounts with different roles complete transfer, review, revision,
revocation, restore, and offline work with actual large media.

## R10 — 3D, rigging, and production integration

- **EB-100:** True 3D transforms/cameras, mesh/material import, environment lighting,
  shadows, 3D text, particles, render passes, and multi-view authoring.
- **EB-101:** SAM 3D Body/MHR native feasibility; editable skeleton/mesh/skin data;
  constraints, IK/FK, animation channels, and artist correction tools.
- **EB-102:** Video temporal fitting, contact/foot stabilization, retargeting,
  bone mapping, coordinate/scale calibration, and GLTF/USD/Blender handoff.
  Single-image body reconstruction is not a finished motion-capture pipeline.
- **EB-103:** SAM 3D Objects candidate for image-to-asset reconstruction, with
  topology/material/scale cleanup and export. Reconstruction is not arbitrary
  character auto-rigging; that needs separate algorithms and evaluation.
- **EB-104:** 3D camera solving, lens data, tracked set extensions, and connected
  Blender rendering/import. Validate camera, scale, time, and pass alignment.

Gate: reconstruct/correct/animate a human rig, retarget a video motion sequence,
and composite a tracked 3D object with editable passes and repeatable handoff.

## R11 — broad production adoption

- **EB-110:** Stable Rust extension contracts, signed/versioned package metadata,
  isolated plugin workers, crash reporting, and video/audio plugin hosting where
  Linux-compatible binaries and licenses exist. Document unavailable platform plugins.
- **EB-111:** Professional delivery queue, retries, interruption cleanup, alpha,
  stems/captions, exact color/channel/timecode metadata, and automated QC reports.
- **EB-112:** Specialist MXF/IMF/DCP/broadcast packages as demanded by supported
  workflows, with representative ingest/export validation and SDK/license records.
- **EB-113:** Intel/AMD/NVIDIA/ARM64 qualification; Wayland/HiDPI/multiple displays;
  tablets, audio interfaces, calibrated video I/O, panels, and jog controls.
- **EB-114:** Local release builds, reproducible artifact provenance/checksums,
  clean installation/update/rollback, supported dependency distribution, and crash diagnostics.
- **EB-115:** Migration campaigns per completed workflow, shortcut training,
  honest compatibility pages, support documentation, and measured activation/retention.
- **EB-116:** Qualify reference jobs for all five professional roles, publish
  limitations, and gather repeat usage before claiming suite-wide replacement.

Gate: a verified supported-workflow/hardware/format matrix, real client job
evidence, clean-machine installs, repeat users, and reversible migrations.

## Coverage of the original gap audit

| Audit gap | Issues |
| --- | --- |
| Safety and recovery | EB-001–003, EB-013–015 |
| Playback/rendering | EB-004, EB-020–026 |
| Ingest/assets | EB-012, EB-016, EB-021–022, EB-032 |
| Editorial | EB-030–035 |
| Multicam/sync | EB-023, EB-060 |
| Color/finishing | EB-024, EB-070–073 |
| Motion/typography | EB-050–052, EB-055 |
| General compositor | EB-020, EB-024, EB-053 |
| Roto/tracking/repair | EB-040–049 |
| 3D/rigging | EB-100–104 |
| Audio post | EB-080–083 |
| Captions/transcripts | EB-061–062 |
| Migration/interchange | EB-006, EB-034, EB-054, EB-083 |
| Extensions/assets | EB-007, EB-016, EB-054–055, EB-094, EB-110 |
| Team/client collaboration | EB-090–095 |
| Delivery/QC | EB-033, EB-064, EB-073, EB-083, EB-111–112 |
| Omarchy/hardware/install | EB-010, EB-081, EB-113–114 |
| Complete-job proof | Every gate, EB-116 |

## Release policy

The first supported cohort is a release sequence, not a permanent feature ceiling.
Do not advertise FCP/Resolve/Premiere/AE replacement until the relevant cohort can
complete its actual jobs. Record input/output fixtures, hardware and software
versions, p50/p95 interaction/playback times, peak memory, render correctness,
audio drift, recovery results, migration losses, and client approval evidence.
Cloud and desktop validation are separate receipts. Local authoring stays usable
without login, a subscription, an inference download, or access to a hosted service.
