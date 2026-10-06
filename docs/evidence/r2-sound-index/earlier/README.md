# Earlier indexed-sound trials

All measurements remain recorded. The first 128-cut six-channel pilot at
`a0a25d7` produced exact PCM and completed device playback, but planning p95 was
5.950327 ms, above the unchanged 5 ms gate.

A subsequent copy started before linking completed and therefore used that same
old executable (`8b623ca459e0cd2299725e834f80f550856e3bdcbc480a55cc7c9e16d5c7e987`).
The mistake was detected by its binary hash; the run is explicitly labeled
`old-binary-repeat`, not attributed to the cropping change. Its passing 4.484397
ms p95 does not erase the first miss.

The correct cropped executable at `7415600` reduced evaluated positions but still
missed: p95 7.545079 ms, maximum 8.270208 ms. PCM remained exact and playback
completed. These observations did not justify raising the gate.

`7dd6a8c` then introduced checked exact rational stepping for single-segment,
unanimated paths, with full evaluation for all other paths and any failed proof.
Its first 128-cut trials pass: six-channel planning p95 2.883974 ms, camera
3.085307 ms, zero PCM error, bounded queues and exact final device samples.
The final combined qualification uses separately frozen executables with the
native longer-cut driver and complete source provenance.

Whole receipts retain every timing observation. The parent PR #33 camera miss is
also still recorded under `../../r2-sound-device/`; no historical failure is relabeled.

The first final native camera driver stopped after its remove/undo/redo/undo
cycle with `Missing current preview frame`. Its trace shows that Undo correctly
invalidated the old preview and the driver read the transient empty request
before the replacement arrived. No playback/export success is claimed for that
attempt. Lab-only commit `064c85d` waits for a current nonempty preview before
performing the observed-state reset; the production app/worker executables are
unchanged. The failed trace and stderr remain here.
