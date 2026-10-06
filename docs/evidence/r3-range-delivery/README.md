# Exact range delivery evidence

Issue #49 / [PR #50](https://github.com/michaelmonetized/editbay/pull/50) follows
PR #48. This qualifies the scoped range workflow on Linux ARM64. Full R2–R11,
sustained playback, physical output and client/adoption gates remain open.

## Actual range and worker results

The prepared runtime `fe76821` completes twelve CLI ranges across real camera
and delayed six-channel AAC projects at 48000 and 44100 Hz. Each matches a whole
master sliced independently by FFmpeg: exact RGBA bytes, float PCM bytes, counts,
channel identities and zero output timestamps. Cases include fractional sample
boundaries, middle sections and the natural sound tail. Original source and
project hashes remain unchanged. `prepared/{camera,six-channel}.json` retain
whole/range receipts and comparisons; `inputs/` retains projects and source hashes.

Sixteen fault/protocol groups pass, including active preparation cancellation,
render cancellation, death, stalled-child cancellation, verification cancellation,
source mutation, destination collision and 26 stale/foreign/malformed protocol
cases. Maximum measured fault retirement is 167.964384 ms against the unchanged
2 s gate. Source mutation only changes a private copied fixture.

## Native workflows

Production app behavior is from `ca7b048`; the final frozen bundle `76d963d`
adds only native driver stabilization and a test-only lint correction. Its binary
and lockfile hashes are recorded. Screenshots were inspected at both 800×600 and
1440×900. The export activity window preserves the compact viewer's actual image.

| Trial | Result | Input p95 | Retirement |
| --- | --- | --- | --- |
| Camera range options and actual file chooser, repeat | Exact pixels/PCM, invalid input blocked, originals preserved | 15.241 ms, 5 viewer inputs | — |
| Six-channel range options and actual file chooser | Exact pixels/PCM, invalid input blocked, originals preserved | 17.290 ms, 5 viewer inputs | — |
| Camera range cancel/death/edit/retry, repeat | Exact slice from revised owner, no failed output published | Edit 10.296 ms (1); viewer 19.950 ms (4) | <=123.496 ms |
| Six-channel range cancel/death/edit/retry | Exact slice from revised owner, no failed output published | Edit 8.691 ms (1); viewer 20.363 ms (4) | <=128.819 ms |
| Camera linked cut, device faults, save/recovery/reopen/full export | Exact independent pixels/PCM and saved/recovered equality | 32.332 ms, 5 edits | See raw device receipts |
| Six-channel linked cut, device faults, save/recovery/reopen/full export | Exact independent pixels/PCM and saved/recovered equality | 28.659 ms, 5 edits | See raw device receipts |

The six-channel full native playback regression also passes with the `de1900b`
bundle: 40 inputs at p95 40.947 ms, 38 cached completed GPU draws at p95 48.990 ms,
and cancellation 125.471 ms. This closes the earlier incomplete short native
playback driver trial, not the two-hour clock gate or physical audibility.

## Failures retained

- `b360f89` passes the camera ranges but its cold six-channel tail has equal sample
  counts and different PCM (maximum absolute error 0.158394814). Starting compressed
  decoding mid-source differs from the whole decode history. Bounded sound pre-roll
  fixes the range output; `prepared-tail.json` and subsequent exact comparisons
  retain that proof. General standalone cold audio seek canonicality remains open.
- The first native driver waits for an unchanged invalid dialog to emit a changed
  state. Its trace is incomplete. The next trial exports correctly but export
  history pushes the compact viewer offscreen; the activity window fixes that.
- `final2` retains moving-dialog/text input driver failures. Its `exits.tsv` has an
  incorrect shell status capture and is **not acceptance evidence**; inspect each
  JSON receipt and error log. The corrected `final3` wrapper captures status before
  running the timestamp command.
- `final3` camera range loses 100 diagnostic records during severe host memory
  pressure; that trial is invalid. Its camera job trial has exact output and
  cleanup but edit input 60.043 ms misses 50 ms. Both remain failed; the separately
  named repeats pass without changing limits or runtime.
- The prepared all-feature test attempt fails with ENOENT while a concurrent Cargo
  build replaces its child executable. Builds are then serialized. Earlier Clippy
  failures concern owned test-driver string comparisons and a test-only remainder
  spelling; both are corrected.

## Local checks and provenance

`de1900b`: 234 default and 235 all-feature tests, plus 7 doc tests in each run,
pass in `final2/`. The production app/CLI code is unchanged afterward.
`76d963d`: formatting, three actual codec integration tests and full
all-target/all-feature Clippy with warnings denied pass in `final3/`.
The only later source changes are driver input synchronization and a test-only
`is_multiple_of` spelling; no audio/render/codec/output contract changed.

Every output MOV stays under the project's artifacts directory; tracked receipts
retain exact file hashes and decoded hashes. Raw native traces are compressed,
not synthesized. No physical photons, speaker/DAC timing, independent user or
client approval is claimed. Preparation remains bounded per worker step, but
range startup grows with its position in the composition.
