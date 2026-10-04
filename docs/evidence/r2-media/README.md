# R2 native ingest evidence

Local Apple M1 Pro ARM64, Omarchy/Hyprland, Linux 7.1.13-3-2-ARCH, Rust/Cargo
1.98.0, 2026-10-04. This completes the selected-stream ingest slice of EB-021
and the ingest-specific ownership/cancellation/crash/resource portion of EB-025.
Both issues and the full production-engine release gate remain open.

## Provenance

Unoptimized native application SHA-256:
`9d18b9bf22b5c8e3f43ad35547be30cee4d54862dc4adb93437ffdb61cf5fad6`.
CLI: `513b8bdcafc5242cfe0c1e6d5fdc06e9f556fcd31726eda14eaed87efd571165`.
Initial native/source qualification driver:
`787bf5a1d82e0f1ff1e608c8f78697a8153d0d1d18ba3e7ca32b76545664fc1a`.
Final recovery driver:
`8f6d216377864c063ea3ebcb4203cf36e439fd443ee959d4b0ccde03fd58e665`.
Lockfile: `d2e4416e9f6b5445ef1c3f54ec8f13cfeb258aed565e077b24d8e783ec8ac873`.
ELF inspection confirms AArch64. Native codec runtime is n9.0.1, with libavcodec/
libavformat 63.1.101, libavutil 61.1.101, libswscale 10.1.101 and libswresample
7.1.101. Frozen binaries and complete private traces remain in
`/var/tmp/editbay-r2-media/`. `source-hashes.txt` identifies the layer's source
inputs; the containing commit is the source revision. Later driver changes only
wait for mapped/focused windows and settled pointer movement; the app/CLI/media
artifacts are unchanged.

## Software checks

Workspace: 89 tests and five SDK documentation examples, zero failures/ignored.
All features: 90 and five examples, with the verified local libtorch selected.
Full all-feature Clippy and fmt pass. The later driver-only changes have their
own lab integration checks, full Clippy and actual native smoke/recovery trials.
Logs are included. No hosted Cargo runner was used.
Log copies omit terminal blank lines and trailing whitespace; original tool
output remains in the local evidence directory. Result lines are unchanged.

Eleven new tests use actual FFmpeg fixtures and packaged Rust application/CLI
workers. They cover explicit multi-stream selection, dropped-picture VFR indexing,
timecode/reel metadata, six native float channels with unknown and declared
layouts, float headroom above 1.0, delayed H.264/AAC, exact seek after EOF and
backward seeks, single PNG/JPEG pixels/alpha, original-file replacement, native
cancel, playlist denial, worker limits and SIGKILL, obsolete UI edits, normal
undo/redo/save/recovery, rejected invalid streams and real document read budgets.
Synthetic fixtures prove invariants; their colors/sound are not client quality.

The real camera AAC initially failed with a false presentation gap and FFmpeg's
skipped-sample timestamp error. Supplying the decoder packet time base fixes the
actual clip; the AAC priming fixture additionally checks exactly 10,143 frames
for a 0.23-second, 44.1 kHz input. Failed ingest did not publish a document edit.

## Native window and portal

Command:

```sh
editbay-lab native-media editbay-studio CAMERA_SOURCE NEW_EVIDENCE_DIRECTORY
```

`native-camera.json` records the actual source chooser, stream selection and
separate codec process. Killing the idle selected-source worker reports a visible
error without another UI action or document change. A subsequent import reads
real source content, then cancels in **126.238 ms**. Normal import succeeds in
**2,284.802 ms**, publishes one source/asset at revision 1, undoes to an empty
document at revision 2, redoes at revision 3 and durably saves that revision.
Renaming/checkpointing revision 4, killing the app and recovering through the
native portal produces a new independent ID with identical assets/streams.
The original saved revision 3 and immutable checkpoint retain their hashes.
Parent application peak HWM is **129,696 KiB**; child virtual memory, CPU and
handle soft limits are observed as 3 GiB, 900 seconds and 512 respectively.

The four included native captures were inspected: actual stream selection,
worker error, imported original-channel/indexed media with acknowledged revision
4, and the separate saved recovery. Empty sequence covers are unchanged by import;
there is no enabled composition playback/export control in this layer.

Saved project SHA-256:
`7a82a45a52eead0bf64e2aaec81a9fef30a39399ab4411a314e6bd678fee4d49`.
Checkpoint: `d1ba3ddcabe2f48132a22459c040995b9703e239398b190b05a63a9d26e5245d`.
Recovered: `ff8d9c7acad7fcd7a0a23ce665134e00b88e542a60c24de7984b7aa33c41afb3`.
Raw projects, source media and complete metadata remain private/local.

## Real source and seek measurements

Command: `editbay-lab media-ingest SOURCE`. Every indexed picture is decoded
sequentially, then selected pictures are sought in reverse order and compared by
RGBA SHA-256. All original sound samples are checked for continuous native sample
boundaries and hashed as little-endian float32. Source identity/checksum is
rechecked after the run. The workload is CPU RGBA8 decode with no frame cache.

| Source | Actual extent | Checks | CPU seek p50 / p95 / max | Peak HWM |
| --- | --- | --- | --- | --- |
| Portrait H.264 1080×1920 | 528 pictures; 0–1,584,000 ticks at 1/90,000; 17.6 s | 25 pixel-equal seeks | 291.103 / 631.905 / 940.575 ms | 89,184 KiB |
| Camera H.264/AAC 1280×720 | 314 pictures; 0–7,850 ticks at 1/600; mono 48 kHz 627,040 samples | 26 pixel-equal seeks; all native sound samples | 174.451 / 469.515 / 472.442 ms | 64,368 KiB |

Portrait source SHA-256:
`872e2e7f3cb19ff13653bea9784c48d78969ceb4b8f7da18d1ea5c7cbaa9a48f`.
Camera: `498ec6c42b6b440886eb761dc4775c994f7578021890ceae3f370ffc6fb3e021`.
Camera decoded native float sound:
`7857156fee51988279dd3fdc0b834b873b06f9d581f3654b9d3d2dadf2c6acc3`.
Picture and sound retain their actual distinct extents; they are not stretched
to equal duration. Both seek p95 values exceed the unchanged 250 ms warm budget.
Exact pixels pass; cache/playback performance does not.

## Recovery regression

The initial full-run attempt stopped before native input at a startup focus
check. A three-trial smoke passed after waiting for mapped/focused windows. A
following attempt recovered the correct content to the portal's default folder
because folder/filename inputs were missed; it is not a passing trial. That owned
test copy was moved to its local evidence directory. Immutable checkpoints and
originals remained intact. Neither incomplete run is counted as qualification.

The final driver verifies portal focus and allows pointer movement to settle
before clicks. A fresh **10/10** smoke passes, with 4,000 catalog documents,
input p95 **13.748 ms**, maximum **14.100 ms**, and 20 UI checkpoint commits
p95/maximum **0.211 ms**. `recovery-smoke.json` contains those receipts.
The separate full regression now passes **100/100** trials, including 50
untitled and 50 saved originals, active/inactive checkpoints, independent native
portal recovery and native reopen. `recovery-qualification.json` records 250
inputs: p50 **11.408 ms**, p95 **26.730 ms**, maximum **88.814 ms**. The 223 UI
checkpoint commits measure p50 **0.165 ms**, p95 **0.928 ms**, maximum **3.951 ms**;
additional automatic checkpoints are retained in the count. Across 17,947 CPU
frames, p50 is **0.868 ms**, p95 **5.063 ms**, maximum **232.159 ms**. Peak parent
HWM is **124,304 KiB**. The unchanged p95 input gate passes; maximum values remain
visible. These are software-injected workspace measurements, not physical input,
power-loss, rendered-media playback or speaker-latency qualification.

Image sequences, hardware decode/texture handoff, thumbnails/waveforms/proxies,
bounded production caches, shared float graph evaluation/preview/export, managed
HDR/display/output color, reverse/shuttle scheduling, two-hour sound drift,
additional hardware and independent accepted production jobs remain open gates.
