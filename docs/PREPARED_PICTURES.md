# Pictures prepared ahead of sound

Published in [PR #40](https://github.com/michaelmonetized/editbay/pull/40), following
[PR #39](https://github.com/michaelmonetized/editbay/pull/39). Issue #38's bounded
preparation dependency has separate [evidence](evidence/r2-picture-clock/README.md).
Dense-cut cache and remaining R2 gates continue beyond this layer.

Play prepares a bounded sequence of resident pictures on the existing Rust viewer
worker. It waits for the first actual GPU draw and the initial future buffer before
starting the existing isolated sound transport. The device callback remains the
clock. Each UI observation selects its exact sequence frame from prepared pictures;
it never changes sound speed to conceal slow picture preparation.

The queue holds at most eight pictures, including an in-flight reservation. Its
dimension-based capacity reserves one additional displayed picture and caps those
pins at 256 MiB, conservatively charging 16 bytes per pixel. This permits eight
queued 720p pictures, seven 1080p pictures and one 4K picture. Existing graph live
texture, source cache, process transport and pending-submission budgets still apply.
Submitted native draws retain their own graph-accounted pins until GPU completion.
This is not a claim that every hardware/format workload plays in real time.

Each preparation queue has a private session and exact half-open frame range.
Only its single current reservation may publish. Pause, seek, edits, tab changes,
viewer cancellation and replacement close the queue and release queued pins.
Cancellation also retires the codec when preparation is in flight, including a
stopped child. Late work cannot publish into a later run. Exact sample-position
pause/resume remains in the shared sound transport.

An empty queue at a required sound-clock frame stops playback and shows a picture
underflow. Initial preparation has a ten-second visible timeout. Frames already
passed by the observed clock are released and counted; their missing draws remain
in the run's evidence. Worker failures retain the viewer's retry path.

## Completed draw evidence

The GPU submission completion callback publishes a scalar receipt into a
preallocated 64-entry atomic queue. A private play UUID, tab, document version,
sequence, frame and request serial identify the draw. Repainting the same resident
picture produces only its first completion receipt. No pixels are read back.

The UI drains at most 64 receipts per observation. A 64-frame bit history
deduplicates bounded reordering and counts distinct completed frames without
retaining the sequence in memory. Overflow, foreign/range-invalid records or
callbacks outside that history invalidate completeness and surface an error.
Cancelled runs cannot claim complete playback. Retired publishers remain tied to
their old queue.

Natural sound end records a monotonic boundary when the UI observes the sound
worker's finished state. Completions after that boundary remain explicit. Missing
frames include the beginning, middle and tail of the run. This boundary is not a
physical DAC timestamp or display-photon measurement, and a frame completed before
the end is not proof that it met every individual presentation deadline.

Diagnostics retain separate numbered `display` records and the bounded aggregate.
The native verifier checks contiguous indices, owners, ranges, diagnostic loss and
agreement with the aggregate. Snapshot-only `skipped_frames` remains a legacy alias
of `accepted_picture_gaps`; neither alone establishes actual display completeness.
`rejected_results` counts UI rejections, while `coalesced_results` counts resident
pictures replaced in the current worker's single result slot before UI polling.
Both discard paths preserve the newest paused request; a scrub verifier must not
assume every obsolete result necessarily reaches the UI.
The opt-in diagnostic file retains its 16 MiB cap. Longer full-frame captures need
explicit bounded rotation before they can supply a two-hour raw trace.

Preparation-request-to-draw time includes intentional time waiting ahead of the
clock. `selection_to_draw_us` measures selecting a due resident picture through its
GPU completion. These are different measurements and must be reported separately.

## Local verification

`editbay-lab native-continuous APP SAVED_PROJECT NEW_DIRECTORY` verifies a complete
short sequence, exact sound end, every available numbered draw record, queue bounds,
first draw before sound startup, source/project preservation and process cleanup.
It reports omissions explicitly and keeps `full_R2_qualified` false.

`editbay-lab native-prepared APP SAVED_PROJECT NEW_DIRECTORY` stops a real picture
codec before preparation, cancels it, then tests exact resume, codec death, picture
underflow and retry. The existing native playback, timeline, save/recovery and
delivery trials remain required. R2's sustained 1080p/4K, seek, cache, hardware and
physical drift gates remain separate from these short software trials.
