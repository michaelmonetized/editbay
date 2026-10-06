# Canonical PCM evidence

Issue #53 / PR #54, following #52. `candidate/source-commit.txt` identifies
frozen `1210983`; `candidate/BINARY_SHA256SUMS` records each packaged executable
and lockfile. Project-local TMPDIR hosts every anonymous cache. These are Linux
ARM64 software/process receipts; no physical audibility or sustained-clock claim.

All nine source suites pass: camera, six-channel AAC and 45-second stereo AAC,
each through canonical late/random access, existing PCM-worker ownership/fault
qualification and sound-block mixing. Canonical reads compare exact float bits
against full FFmpeg decode, including padding and pins retained after clear.

| Fixture | Cold late p95, 10 trials | Warm random p95, 320 reads |
| --- | ---: | ---: |
| Camera | 524.125 ms | 0.409 ms |
| Six-channel | 204.953 ms | 0.773 ms |
| 45-second stereo | 177.637 ms | 1.755 ms |

Camera whole masters at 48/44.1 kHz match the pre-change masters, and six ranges
match independently decoded slices. The six-channel run fails comparison of its
44.1 kHz whole master with the pre-change master: 66,155 sample values differ,
maximum error 0.151653, first output frame 126915 and last 143853. Raw failure and
the localized difference are retained. Frozen `27adb6e` independently decodes
the AAC source through FFmpeg to float WAV, verifies unchanged PCM bits and
substitutes it into the same typed graph with the captured 4976-sample origin
offset preserved exactly. The new whole master matches that reference bit for
bit; the old master fails by the same 66,155 values / 0.151653 maximum. This
isolates the old compressed decoder-history defect; it does not claim a separate
resampler implementation. The first oracle harness assumed zero-origin media
and refused this source; that failed run is retained under `oracle/`.

All twelve camera/six-channel ranges at 48/44.1 kHz match independently decoded
slices of their new whole masters. Three whole masters retain their earlier PCM;
the six-channel 44.1 kHz master corrects the documented tail defect.

All six native runs pass: camera and six-channel prepared-picture faults, device
controls/timeline/save/recovery/export, and full playback/seek/pause/replay/fault
workflows. Camera/six-channel input p95 is 20.709 / 19.558 ms, completed draw p95
26.916 / 19.411 ms, and active cancellation 119.318 / 121.596 ms. Both 800×600
screenshots were inspected. The camera cold-tail trace records an actual partial
source preparation (197568 / 627040 frames), then exact completion and playback.
Six-channel preparation fits one bounded native step; no partial record is claimed.

The frozen implementation passes 246 default / 247 all-feature tests and seven
doc tests each. A subsequent late AAC resampling regression test passes as well.
Initial Clippy rejected test-module placement; the correction passes final
all-target/all-feature Clippy with warnings denied, the five-test sound suite
with all features, and formatting. All original and final logs are retained.
Preparation ahead of future cuts is tracked in #55. Full R2/R8, hardware and
R3–R11 release/adoption gates remain open.
