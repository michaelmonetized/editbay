# Canonical PCM evidence

Issue #53 / draft PR #54, following #52. `candidate/source-commit.txt` identifies
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
the localized difference are retained. Independent uncompressed-source checking
is in progress; this changed master is not yet accepted as a corrected result.

Native workflow and full workspace receipts are pending. Preparation ahead of
future cuts is tracked in #55. Full R2/R8, hardware and R3–R11 release/adoption
gates remain open.
