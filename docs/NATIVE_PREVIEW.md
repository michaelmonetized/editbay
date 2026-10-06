# Native picture workspace

The native workspace creates editable picture and sound sequences from imported video and
draws the shared SDR GPU graph on its actual window device. It uses the packaged
Rust picture worker, the core command/undo path and the existing save/recovery
foundation. This is paused picture authoring and viewing. Sound scheduling,
sustained playback and shared delivery remain R2 work.

## Use it

Open a project, import a video, select its actual video stream, then expand the
source, choose an imported sound stream or **Picture only**, then choose
**Create sequence from video**. A sole imported sound stream is selected initially;
multiple sound streams require a choice. Select the resulting sequence
and use Previous frame, Next frame, the frame number or the scrubber. Save,
Undo, Redo and recovery use the same document as the CLI and automation.

Cancel viewer stops the underlying codec and GPU job. Retry viewer creates a
fresh job after cancellation or failure. Errors appear in the workspace; there
is no enabled playback or export control without its implementation.

## Source authoring

`sequence_from_video` returns ordinary composition and sequence commands for one
undo group. Background validation captures the tab, session, document revision
and save generation. A changed owner or view rejects its result before mutation.
The first empty sequence profile is reused; later sources create distinct ones.
Undo removes the authored composition while preserving imported source records.

Uniform indexed timestamps use their exact rational rate. True VFR keeps every
original presentation timestamp and uses the project's sequence rate. The rate
must produce whole source ticks per sequence frame; incompatible timing fails
explicitly. Duration rounds up to a whole frame. Integer frame starts retain
their natural source times; a final partial interval ends at the actual source
end. Sound uses the separate exact clock described below.
Sample aspect is converted to square-pixel composition width. Unsupported source
rotation or color interpretation remains a visible graph error.

`sequence_from_video_with_audio` adds reciprocal picture/sound clip links and a
nested source-sound composition in the same undo group. Sequence zero is the
video's original presentation origin. The nested clock is the least common
multiple of the picture rate numerator, both source time-base denominators and
the original sample rate. Every source sample center remains exact, including
44.1 kHz with fractional frame rates. Sound is never fitted to a rounded picture
duration. Delayed starts and partial final-frame tails are silence; later sound
extends the sequence beyond the last picture. Earlier sound remains intact in
the nested source composition but lies before this sequence's playback origin.
A selected stream ending entirely before the picture starts fails explicitly.
Clock/range overflow fails before document mutation. Imported assets, source
metadata, original channel identities and sample rate remain unchanged.

The viewer shows empty picture regions over its transparency checkerboard.
Sound authoring and offline PCM verification do not enable device playback.

## Worker and GPU ownership

The preview actor compiles its immutable evaluation snapshot, native display
pipeline and codec provider off the input thread. Its request and result
mailboxes each hold one value. Rapid scrubbing replaces queued requests; a
completed superseded request cannot replace the latest requested frame. The
previous valid picture remains visible with its frame number while a new one
loads. A scrub does not interrupt an active codec request; Cancel and ownership
changes cancel the lifetime token and kill/reap the codec when needed.

Only one actor is active and one may be retiring. Replacement waits for retirement
without blocking the UI. Tab switches, edits, save generation changes, close and
welcome navigation invalidate the old actor. Its idle health check notices a dead
codec even without another frame request. Retry binds a fresh process and token.

`GraphRenderer::with_device` uses eframe's actual device and queue. The same typed
source, nested composition, affine, over and opacity operations produce resident
FP16/FP32 premultiplied linear textures. Display conversion is explicit; the UI
performs no pixel readback. A native draw accepts only a matching device and the
declared BT.709/sRGB boundary. Every encoded draw retains its charged picture
until that exact submitted command completes, or until the unsubmitted command
is dropped. Consumer and GPU submission pins remain in the graph's live budget.

The native shader preserves upright source coordinates, scales unfilterable float
textures with explicit bilinear sampling, and composites transparency over an
opaque checkerboard. sRGB surface formats apply the inverse transform before the
surface's hardware encoding. This is an 8-bit SDR surface contract; calibrated
monitor output, HDR and other hardware require their own qualification.

## Qualification

```sh
editbay-lab native-preview APP_BINARY SOURCE NEW_EVIDENCE_DIRECTORY [full|half]
```

The Rust driver seeds a project with real native source indexing, then exercises
actual window controls for sequence creation, cached stepping, coalesced scrubbing,
worker death/retry, active cancellation, Undo/Redo, Save, kill/restart, native
recovery copy and explicit original reopen. It verifies original source, saved
project and checkpoint hashes, independent recovery identity and actual native
GPU completion. When sound is present, the seed imports one explicit sound
stream, the UI authors the linked graph, and the driver compares every saved
and recovered output sample against independent FFmpeg PCM through the packaged
worker. It also presents the recovered final frame. The seed does not replace
the separate native UI-ingest evidence.
The driver supports the current Flea native file chooser and the older Synchro
chooser. It reads picker state and control geometry, then injects actual input;
those observations grant no document mutation authority.

The scoped budgets are input-injection to request acceptance p95 <=50 ms, cached
request to GPU draw completion p95 <=250 ms and active cancellation <=2 s.
Forty alternating native requests supply input samples; the first two fill the
cache and the remaining 38 supply cached draw samples. Every sample and the
injector's explicit 8 ms button event delay are recorded. Request acceptance is
timestamped when the UI successfully queues work, independently of later
diagnostic publication. GPU completion times the actual submitted draw, not
physical display photons. These gates do not close full R2 playback, cold/warm
uncached seeking, sound drift, export or the device matrix.

Actual GPU regressions compare both precisions across RGBA/BGRA linear and sRGB
surfaces against independent orientation, scaling, transparency and color
references, within 1.2 output code values. Separate ownership checks reject
foreign devices/workers and retain allocations until submitted work completes.
Native screenshots at 800x600 and 1440x900 document real window behavior.

Measured receipts and retained failed candidates belong under
`docs/evidence/r2-native-preview/`; private media, binaries and raw traces stay
under ignored `artifacts/` in this checkout. Full R2 and enterprise/GTM gates stay
open until the roadmap's complete workloads pass.
