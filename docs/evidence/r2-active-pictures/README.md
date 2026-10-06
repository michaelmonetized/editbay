# Reuse proven transparent picture inputs

Issue [#34](https://github.com/michaelmonetized/editbay/issues/34) follows
[PR #35](https://github.com/michaelmonetized/editbay/pull/35). The shared Rust
preview/export change is `9e8fcf5`; final production timing diagnostics are
`0d7e8c4`. The native driver adds a verified focus check before saving in
`69aeca9`; the frozen application and CLI remain the `0d7e8c4` artifacts.

## Correctness and ownership

The actual FP16/FP32 GPU test evaluates 128 sequential solid/Over layers at four
forward/reverse-boundary positions. Independently expected premultiplied pixels
agree with only **five dispatches and five cached textures total**. Four held
outputs keep four payload charges after cache clearing; releasing each result
releases its charge. Different nested geometry, transforms, partial/unity opacity,
declared output gamut/transfer and transparent pixels also pass. Invalid transforms,
oversized solids, unsupported operations, HDR and stale receipts still fail.

**196 default + 7 doc tests**, **197 all-feature + 7 doc tests**, formatting and
workspace/all-target/all-feature Clippy pass at `0d7e8c4`. The final driver-only
focus change passes all-target/all-feature lab Clippy and the actual native trial.
Logs, full source/lockfile and executable hashes, compiler/host identity and
commands are retained. Project artifacts and temporary files stay under the
project; original media remains unchanged.

## Native observation and open gates

The camera workload has 128 one-frame cuts into a real 1280x720, 24 fps source;
source selections jump by 37 frames. The six-channel AAC fixture has 320x180
pictures. Neither is a 1080p/4K or long-duration hardware qualification. The actual
window uses the **Apple M1 Pro, Vulkan, Honeykrisp/Mesa 26.2.3** adapter.

The parent camera trial recorded 3,194 dispatches, 3,158 evictions and 98 skipped
frames, displaying 118 when sound requested 127. The first simplified run recorded
169 dispatches, 133 evictions and 69 skips. Final camera records **187 dispatches,
151 evictions and 65 skips**, displaying 126 at that same sound boundary. Dispatch
totals depend on completed pictures; the fixed-position GPU test isolates the
optimization. Skipped frames are still a failed sustained-playback result.

All four final camera edits pass the 50 ms input gate: p50 **28.007 ms**, p95/max
**46.472 ms**. Its four seek request-to-GPU-completion observations are **120.940,
12.423, 301.906 and 272.196 ms**; the latter two exceed the existing 250 ms warm
target. Native viewing performs zero CPU readbacks. The final observed GPU cache
is 530,841,600 bytes, raw picture cache 228,556,800 bytes, and pending submissions
zero. These allocation counters are not complete process/hardware peak memory.

The earlier instrumented camera run misses input at **50.865 ms** and skips **99**
pictures. Among 28 consecutive observed playing-picture intervals, source retrieval
is **148.561 / 359.694 / 443.710 ms** p50/p95/max; temporal preparation is
**0.523 / 3.235 / 3.533 ms**; GPU completion is **3.777 / 5.282 / 6.127 ms**.
Source work consumes 4,965 of 5,168 ms inside rendering. That failed run is retained
alongside the earlier pilot and timing runs, with separate executable hashes.

The first final six-channel attempt reaches the exact sound end and displays all
pictures, then receives no Save key event in the owned window and times out. Its
raw trace and untouched saved copy are retained under `earlier/final-native-six`.
The driver now explicitly verifies focus before sending Save; this is a driver
fix, not a change to application persistence or rendering.

The completed six-channel trial has input p50 **34.008 ms**, p95/max **45.859 ms**;
four seeks complete between **14.436 and 25.020 ms**. It reaches exact sound sample
235,436 and displays frame 127, with **one skipped picture, 192 dispatches and
zero evictions**, versus the parent's zero skips and 8,976 dispatches. Final GPU/raw
cache charges are 88,473,600 / 10,137,600 bytes, with no pending submissions or
native CPU pixel readbacks. Its missed picture remains reported.

Both completed native trials preserve all 128 linked clips through remove, undo,
redo, undo, save, independent recovery and reopening. Native chooser exports
retain every RGBA and original-channel PCM byte. Both whole MOV file hashes are
identical to the parent masters independently verified against every source slice:
camera **471,859,200 RGBA / 1,024,000 PCM bytes**, six-channel **29,491,200 RGBA /
6,150,144 PCM bytes**. These are byte-identity comparisons to retained independent
parent proof, plus fresh full decode against the new renderer's output receipts;
the source-slice comparison was not rerun. Native 1440x900 camera and 800x600
recovered six-channel screenshots were inspected.

The next source-decoding dependency is
[issue #36](https://github.com/michaelmonetized/editbay/issues/36): bounded forward
advancement without a keyframe restart for each small skipped-picture gap. Dense
random edits still need further decode/cache work. Full R2, hardware-matrix,
physical-audibility, two-hour drift, client and independent-user gates remain open.

Readable receipts omit repeated control rectangles and replace repeated clip
arrays with their counts. Compressed complete receipts, raw native/focus traces
and editable saved/recovered manifests retain every observation. Full media stays
in `artifacts/active-pictures`; `master-files.sha256` records whole-file identity.
