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
native options/chooser windows and worker fault injection. Receipts are added
after execution; implementation alone does not establish these gates.

Delivery queues, additional profiles, sustained playback, physical timing and
complete R2/R3/R7/R11 production acceptance remain open.
