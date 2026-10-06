# Exact range delivery

Issue #49 adds frame range selection to the shared native/CLI lossless MOV path.
Choose **Export → Frame range**, enter the first frame and excluded end frame,
then choose a new destination. Full sequence remains the default.

```sh
editbay export-range PROJECT COMPOSITION_ID NEW_MOV START_FRAME END_FRAME [SAMPLE_RATE]
```

At composition rate N/D and output sound rate S, pictures [A,B) select sound
samples [ceil(A*S*D/N), ceil(B*S*D/N)) from the unchanged composition grid.
Output timestamps start at zero. The selected sound duration can differ by one
sample from rounding (B-A) at zero; that difference is preserved. Source picture
positions and sound interpolation retain their original absolute time.

The parent and worker reject empty, reversed, out-of-bounds and overflowing
ranges. The returned profile records its original first frame, selected frame
count, sound rate and unchanged channel identities. Parent validation rejects a
receipt for another range. Existing bounded rendering, private file verification,
source ownership, cancellation, retry and non-overwriting publication apply.
The options capture document ownership before the native file chooser opens.

Qualification uses actual camera and delayed six-channel media, full references
at 48 and 44.1 kHz, independent FFmpeg trim/atrim decode/hash comparisons, actual
native options/chooser windows and worker fault injection. [Actual receipts](evidence/r3-range-delivery/README.md) pass the scoped ranges,
native workflow, worker faults and checks, retaining all earlier failures.

Delivery queues, additional profiles, sustained playback, physical timing and
complete R2/R3/R7/R11 production acceptance remain open.

The first actual six-channel tail trial found equal sample counts but different
PCM after a cold compressed-audio seek (maximum absolute error 0.158394814).
The failed outputs and trace are retained. A ranged export now prepares its
preceding sound with the same composition-frame/block boundaries as a whole
export before writing any selected samples. Each worker preparation step renders
at most 4096 samples, reports exact preparation counters and remains cancellable.
No preceding pictures are rendered or written. Preparation time grows with the
range's start; a later canonical PCM store can remove repeated decode work without
changing this exact-output contract. This does not claim that arbitrary standalone
audio seeks are independently canonical yet.
