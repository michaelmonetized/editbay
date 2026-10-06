# Linked timeline qualification

Issue [#29](https://github.com/michaelmonetized/editbay/issues/29), stacked on
shared delivery [PR #28](https://github.com/michaelmonetized/editbay/pull/28).
Runtime source: `0978a6f`. The compiled-input manifest, frozen binary hashes,
build log and machine receipt identify the exact local build. Final trials ran
**2026-10-06 03:22:33–03:23:25 UTC**, without own builds during timing. Project,
temporary storage, native state and masters remained under this worktree.

## Native workflow

Both real camera and synthetic delayed six-channel sources complete source I/O
marks, create cut, keyboard append, keyboard split, linked remove, ripple source
trim, undo/redo, cut-boundary GPU viewing, real streaming playback, pause, native
save, recovery, chooser export and reopening the recovered cut. The driver checks
the actual record sequence ID, version, GPU completion and zero viewer readbacks.
The recovered cut also draws through the 800×600 window. Screenshots were inspected.

Each result has two linked groups: source frames **[16,28)** at record **[0,12)**,
and source **[42,65)** at record **[12,35)**. The six-channel timecode picture visibly
shows source frame 42 at record 12 and source 27 at record 11. Native undo restores
the second source range [40,64); redo restores the exact trimmed clip identities.

| Source | Output | Original-channel sound | Five edit inputs p95/max |
| --- | --- | --- | ---: |
| Camera IMG_0001.MP4 | 1280×720, 35 pictures, 24/1 fps | 70,000 mono FC samples, 48 kHz | 24.354 ms |
| Delayed six-channel AAC | 320×180, 35 pictures, 24000/1001 fps | 70,070 frames of FL/FR/FC/LFE/BL/BR, 48 kHz | 22.383 ms |

These retain the predeclared 50 ms native input gate. Ten software-injected inputs
are small workflow samples, not sustained latency or physical input measurements.
The monitor is stereo and its clipping is visible for the deliberately loud
six-channel fixture. Delivery retains all original channels and float headroom.
There is no physical-audibility claim.

## Independent media and persistence

The production worker decodes and verifies its own anonymous master before
publication. The lab independently decodes the published file. Separately,
`timeline-compare` decodes the corresponding ranges from the previously qualified
uncut source master using installed FFmpeg. Concatenated slices agree with every
output byte: **129,024,000 camera RGBA bytes / 280,000 PCM bytes** and
**8,064,000 six-channel RGBA bytes / 1,681,680 PCM bytes**.

The actual CLI then moves the two groups into reverse source order without
flattening source compositions. Three captured-version requests per source each
succeed once; replaying all six stale requests fails and preserves saved bytes.
Reversed exports match independently selected picture/PCM slices exactly,
including backward source access. Source project and original media hashes remain
unchanged. Saved and recovered cuts have identical picture, PCM **and whole-file
hashes**. `gates.json` verifies those equalities. Editable saved/recovered/reordered
project manifests are retained beside the receipts; their media paths refer to
the preserved local sources.

The camera recovered delivery worker peaks at 288,544 KiB. See individual delivery
receipts for timings and process memory. These short cases do not establish the
long-workload or combined CPU/GPU 4 GiB gate. High-frequency delivery observations
are compressed alongside readable summaries; native traces and focus receipts
are also retained as gzip JSONL.

## Software checks and limits

- **179 default tests + 7 doc tests** and **180 all-feature tests + 7 doc tests**,
  zero failures or ignored tests; locked fmt and all-target/all-feature Clippy pass.
- Six core timeline checks cover exact inserted and fractional sample centers,
  gaps, linked identities, ripple/move, overlap/overflow/cycle/unknown-graph/profile
  rejection, undo/redo, save and recovery.
- Two native ownership checks cover normal atomic commit, revision change, tab
  switch, closed tab and hidden workspace. The CLI regression verifies captured
  revision, saved bytes and malformed-request rejection.

Read the [contract](../../TIMELINE_AUTHORING.md). This first layer supports one
linked V1/A1 pair, matching dimensions/rates, unchanged channel layouts, and the
existing bounded sound paths. Advanced trim modes, overwrite, mixed-profile
conform/routing, timeline indexing, multiple targeted tracks and simultaneous
source/record monitors remain open. Full R2/R3–R11, two-hour hardware drift,
other GPUs, independent-user proof and client acceptance remain open.

The [earlier trials](earlier/README.md) retain a reproduced desktop-portal realtime
limit failure and a driver identity-check gap. Restarting the user portal restored
actual callbacks. [Issue #30](https://github.com/michaelmonetized/editbay/issues/30)
continues with sound-device process isolation so backend failure cannot kill the
editor. Final receipts do not hide either earlier failure.
