# Dense-cut exact picture preparation

Issue [#41](https://github.com/michaelmonetized/editbay/issues/41) follows
[PR #40](https://github.com/michaelmonetized/editbay/pull/40). The qualified runtime
and frozen app/CLI/lab binaries are from
`3ecb330` (full identity in [source-commit.txt](final/source-commit.txt)).
See [contracts](../../EXACT_PICTURES.md), [command exits](final/exits.tsv),
[binary hashes](final/binaries.sha256) and the complete compressed native traces.
The final four commands ran on 2026-10-06, 08:02:02–08:03:39 UTC.

This qualifies the bounded exact source cache and the recorded **128-cut** native
workflows on Linux ARM64, Apple M1 Pro / Honeykrisp / Mesa 26.2.3 / Vulkan. It
resolves the earlier camera picture underflow and dense seek/input misses on these
workloads. **Full R2–R11 and release/adoption acceptance remain open.**

## Final measurements

| Observation | Camera, 1280×720 / 24 | Six-channel AAC fixture, 320×180 / 24000/1001 |
| --- | ---: | ---: |
| Unique exact source pictures / bytes | 128 / 471,859,200 | 44 / 10,137,600 |
| Isolated preparation | 2651.143 ms | 52.031 ms |
| Checked disk read p95 / maximum | 24.034 / 90.276 ms | 1.179 / 1.691 ms |
| Independent raw-pixel matches | 128 / 128 | 44 / 44 |
| Native GPU draws by observed sound end | 128 / 128 | 128 / 128 |
| Missing / rejected / overflowed draw records | 0 / 0 / 0 | 0 / 0 / 0 |
| Selection to GPU completion p95 / maximum | 15.195 / 29.013 ms | 14.067 / 19.946 ms |
| Four native seeks, request-to-draw p95; gate 250 ms | 71.087 ms | 43.238 ms |
| Remove/Undo/Redo/Undo input p95; gate 50 ms | 44.006 ms | 47.288 ms |
| Cancel stopped preparation; gate 2 s | 125.366 ms | 123.758 ms |
| Kill preparation worker and retire; gate 2 s | 16.544 ms | 16.089 ms |
| Edit invalidates stopped preparation; gate 2 s | 132.518 ms | 139.854 ms |
| Sampled app/descendant RSS peak; gate 4 GiB | 550,272 KiB | 298,560 KiB |

The six-channel fixture contains 44 actual video pictures and a longer delayed
sound tail. Its 128-cut sequence includes transparent tail intervals. The four
seek observations include three source pictures and one such interval. Every
rendered timeline picture is counted; source preparation does not invent frames
outside the original video index.

Memory is sampled every 50 ms over the actual app process tree, including codec,
sound and delivery children. RSS can count shared pages twice and excludes
unmapped filesystem page cache and unreported device/driver allocations. The
existing decoded/mapped/GPU budgets remain unchanged. These measurements are not
a complete physical GPU-memory qualification. Every observed child is absent
after retirement, and the private cache directory has no files.

The isolated runs use both packaged CLI and app codec endpoints. Fresh children
read every prepared picture without opening a decoder. The camera run exercises
56 resident cache evictions while retaining the disk pack. Pins keep all reserved
bytes/entries charged, and dropping the last owner releases both totals to zero.
Stale document attachment, killed readers, stopped-reader cancellation and
partial-preparation cancellation pass. Unit fixtures independently cover corrupt
and truncated pixels, writable/named descriptors, foreign versions, duplicate or
malformed indexes, altered color/alpha/rotation, changed/missing sources, live
allocation pins, source-order planning and nested reverse time.

## Complete native path and delivery

Both actual windows complete preparation, cancellation, failure/retry, four seeks,
edit invalidation, history, preparation of the new revision, full sound-clock
playback, Save, separate recovery, native chooser export, recovered reopen and
800×600 viewing. The actual full-size camera and compact six-channel captures
were inspected. The final sound position equals the exact captured endpoint.

Both masters are byte-identical to the earlier independently source-slice-checked
long-cut masters:

- Camera: `2b998a7824cf19b7c88aeb5ee07388819521ccd9f3ff9842605e7fe437404a72`.
- Six-channel: `77955d21f2bf4698d820f88fe067714c844db1449d04a5347cf08ac61c6a04aa`.

Fresh independent full-master decoding also matches the current renderer's RGBA
and original-channel float PCM receipts exactly: camera 471,859,200 picture bytes
and 1,024,000 PCM bytes; six-channel 29,491,200 picture bytes and 6,150,144 PCM
bytes. The earlier per-cut source-slice comparison is reused through the identical
whole-file hash; it is not represented as a fresh run. Original project and source
hashes are rechecked. Sources and masters stay outside Git.

## Retained earlier candidates

- `bd49767`: all 128 camera raw pictures matched, but unoptimized ARM64 SHA-256
  made disk reads 134.450 ms p95 and preparation 16.409 s. Native playback was not
  qualified on that build. Development-profile SHA-256 optimization preserves the
  same algorithm, digest format and full checks.
- `bee0f47`: optimized raw pixels pass both fixtures. Both native trials stop at
  retry because a wrapped error label overlaps the preparation button. The actual
  [overlap screenshot](earlier/optimized/overlapping-error.png) and failed traces
  remain. Status and error text now occupy their own rows.
- `122ab51`: six-channel seeks/faults/save/recovery/export complete, but a sound
  underrun and 71.299 ms Remove miss remain failures. The camera driver mistakenly
  accepts the preceding failure while a retry click is still being delivered.
  Monotonic preparation-attempt observations now distinguish retries. Core and
  sound computation use optimization level 2 in the development profile; no
  algorithm, buffer, deadline or release-profile limit is relaxed.
- The first final-check wrapper receives status 143 after complete passing default
  test output, before recording the command exit. Cause is unconfirmed. Its log
  is retained, and independently exited checks supersede it.

The earlier observations remain in `earlier/`; the final code and its recorded
measurements supersede them. No failed run is relabeled as a pass.

## Evidence boundaries

The uncached native playback regressions also pass both source workflows, with
40 actual inputs each: camera input p95 20.668 ms and 38 cached draws at 19.601 ms
p95; six-channel input p95 22.778 ms and 38 cached draws at 28.909 ms p95. Active
cancellation takes 121.413 ms and 122.506 ms respectively. These use the same
`3ecb330` app and lab binaries; their full receipts and traces are in `regressions/`.

The separate prepared-playback fault driver initially races Retry viewer against
an unchanged frame selection. The app has already drawn that frame, and the
driver incorrectly waits for another draw. Those failures remain in
`earlier/prepared-driver/`. Commit `9aeab30` changes only the lab acknowledgment:
it waits for the retried current frame before selecting the next target. The
frozen corrected driver, its source/hash and passing Clippy exit are recorded in
`regressions/driver/`; the app remains the identical qualified `3ecb330` binary.
Both reruns pass exact sample resume, source preservation and owned-child reaping.
Camera/six-channel stopped-preparation cancellation takes 123.716/127.837 ms,
playing-worker death retires in 91.068/22.727 ms, and stopped-worker underflow
retires in 618.393/616.897 ms. These are observed runs, not isolated-machine claims.

Local locked validation on the same runtime source passes **213 default tests +
7 doc tests**, **214 all-feature tests + 7 doc tests**, formatting and workspace /
all-target / all-feature Clippy with warnings denied. [Check exits](checks/exits.tsv)
and compressed complete logs are retained. All-feature inference dependencies use
the already installed native libtorch route; no hosted checks are substituted.

Numbered GPU completion receipts prove that the requested pictures completed
draw submission work by the UI's observed sound end. They do not measure photons,
physical audibility, every individual display deadline, or two-hour hardware
drift. The software monitor retains explicit clipping counts; original delivery
channels and samples remain exact. Sustained 1080p/4K hardware coverage, wider
formats, production jobs, independent users and the remaining roadmap are open.
