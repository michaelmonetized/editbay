# Source preparation evidence

Issue #55 / PR #56, based on #54. Frozen production executables come from
`55f9c355e11dbfbff61f1a9b50e6971b375a97e7`; candidate SHA256SUMS identifies them
and Cargo.lock. `678050ae20ed54d293b7165f81f2cf5015ca2a27` changes only the lab's
preparation-cancellation trigger and documentation. Its separate driver hashes
are retained. Project-local TMPDIR contains anonymous cache storage. Linux ARM64,
Apple M1 Pro, Mesa 26.2.3 Honeykrisp/Vulkan, existing CPAL ALSA default device.
This is software/process evidence, not physical audibility or display timing.

| Workload | Camera | Six-channel AAC |
| --- | ---: | ---: |
| 256-cut source preparation | 62.058 ms / 4 steps | 15.597 ms / 1 step |
| Subsequent render p95 | 3.606 ms | 5.782 ms |
| Native playback input p95 | 20.556 ms | 20.900 ms |
| Native completed draw p95 | 19.132 ms | 32.465 ms |
| Active native cancellation | 124.949 ms | 121.153 ms |

Both 256-cut timelines match reference PCM exactly, perform no subsequent source
decode or store growth during rendering, finish at the exact device-observed end,
and reap owned processes. Four whole masters and twelve ranges preserve #54's
corrected picture/PCM output, including the six-channel 44.1 kHz tail.

All eighteen stream/device faults pass; maximum retirement is 614.681 ms. Fourteen
initial delivery fault/protocol groups pass. Two preparation-cancellation trials
fail because the original lab trigger waits for a partial progress event after
one-step preparation has already completed; no cancel was sent and both private
qualification outputs completed. Their raw failures remain. The corrected driver
cancels after worker plan admission (before the first step), with no publication,
retirement 26.752 / 6.251 ms, and full cleanup. The actual-worker tests separately
cover cancellation after real partial decoding and retry.

All six native preparation/device/playback workflows pass. Both 800×600 playback
captures were visually inspected. Three native range workflows pass initially.
Camera range-job cancellation, kill, edit invalidation, retry and exact output all
pass, but viewer input p95 is 157.870 ms. Its trace records recovery publication
on the UI thread for 236.380 ms inside a 239.610 ms frame. The unchanged repeat
passes at 22.740 ms, with retirement <=123.890 ms. Both runs remain in
`candidate/native-ranges/`. Issue #57 tracks moving durable recovery publication
off the UI; the repeat does not make the original responsiveness failure pass.

Fresh camera/six-channel six-second pilots, a 180-second stereo pilot and resumed
device fault trial pass. The long pilot finishes in 180.648 s with maximum
backend-relative drift 0.472165 ms, host-adjusted drift 2.600053 ms and combined
peak RSS 133936 KiB. This does not qualify the two-hour gate or erase earlier
sustained failures.

Final workspace checks at `678050a` pass: 250 default / 251 all-feature tests,
seven doc tests each, all-target/all-feature Clippy with warnings denied, and fmt.
Initial invalid test fixture and corrected focused checks are retained. Seven
actual-worker preparation tests and six renderer integration tests pass. All
source/project preservation and bounded ownership gates retain their checks.
Full R2/R8 sustained/hardware and R3–R11 release/adoption gates remain open.
