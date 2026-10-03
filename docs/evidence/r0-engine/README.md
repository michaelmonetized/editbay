# EB-004 native engine feasibility

Measured locally on 2026-10-03: Linux ARM64, Apple M1 Pro, Vulkan/Honeykrisp,
Mesa 26.2.3, FFmpeg n9.0.1, Rust 1.98.0. Omadesign's pinned reference is
`c4813ef9a9628274778047e880a3aff6ea98d199` (eframe 0.36.1 / wgpu 30).
The local optimized ELF is ARM aarch64. `provenance.sha256` records the binary,
lockfile and implementation files. No source media is included in this repository.

## Results

| Workload | Frames | Decode p95 | FP16 upload/compose/readback p95 | FP32 upload/compose/readback p95 | Peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Synthetic H.264 1920x1080 30 fps | 90 | 0.700 ms | 10.533 ms | 3.350 ms | 187.1 MiB |
| Synthetic H.264 3840x2160 30 fps | 60 | 5.412 ms | 63.296 ms | 35.196 ms | 442.0 MiB |
| Existing Omadesign portrait render, 1080x1920 30 fps | 60 | 2.015 ms | 10.384 ms | 4.064 ms | 197.3 MiB |

FP16 maximum linear error is 0.0004881; FP32 is 0.000000239 against the independent
CPU transfer/exposure/alpha-over evaluator. Both meet the predeclared R0 color
budgets. Normalized channels include values above 1 before the output transfer.
GPU tests additionally cover straight alpha, all code values, unaligned row widths,
both sRGB and BT.709 transfer functions, and repeat evaluation.

The actual default ALSA output reports stereo F32 at 44.1 kHz. Five seconds of
48 kHz decoded audio ran through 155 device callbacks. The corrected clock
selected 296 pictures and dropped 4 of 300 in the existing 720p60 recording;
scheduling lateness was p50 0.528 ms, p95 1.042 ms, maximum 2.433 ms. Backend
reported latency was zero. This is measured device callback/scheduling evidence,
not physical audibility, presentation, source A/V offset or long-duration drift
qualification. The initial buffer-stepped clock selected only 154 pictures;
interpolation is essential. The remaining dropped pictures keep R2's playback
qualification open.

An isolated Rust worker exported 60 real portrait pictures as FFV1/Matroska in
6.420 s, reopened all 60 pictures, and matched every decoded RGBA byte against
the composed codec input before publication. Peak RSS was 361.2 MiB. The source
hash stayed unchanged. Cancellation at 100 ms stopped the underlying process in
1.469 ms and published no destination. Temporary output/report files are owned
by the parent and cleaned on failure or cancellation. Publication refuses an
existing destination, including a competing file created during rendering.

The workspace has 26 passing tests, zero ignored: 19 project/recovery/CLI tests,
3 native codec tests, 1 actual GPU comparison, 2 process export/cancellation
tests, and 1 callback-clock invariant test. Clippy with warnings denied and
formatting pass. The CLI test kills a real save process in its temporary-file
publication window, then validates the project, existing checkpoint and restart.
This does not substitute for the future native-window kill/recovery gate.

## Reproduce

Build locally with `cargo build --release --workspace --locked`. On this machine,
`CARGO_TARGET_DIR=/var/tmp/editbay-build` keeps builds off the nearly-full home
filesystem. `editbay-lab --help` lists the actual probe commands:

```sh
editbay-lab probe SOURCE 90
editbay-lab playback SOURCE_WITH_AUDIO 5
editbay-lab export SOURCE NEW_DESTINATION.mkv 60
editbay-lab cancel-export SOURCE NEW_DESTINATION.mkv 3600 100
```

The synthetic workloads use FFmpeg's `testsrc2` at 1920x1080 or 3840x2160,
30 fps, libx264 ultrafast CRF 18 YUV420P, lasting 3 s or 2 s respectively.
The codec regression fixture adds 44.1 kHz PCM and delayed H.264 B frames at
30000/1001. Native decode must drain both the video decoder and sound resampler.
JSON receipts contain the exact input paths, source hashes, profile, sample counts,
timings, memory and limits. Large outputs remain in
`/var/tmp/editbay-evidence/r0-engine/`.

## Scope and decisions

- FFmpeg decode is CPU-only, with two codec threads and bounded decoded picture/
  sound buffers. The adapter is a narrow C codec boundary; ownership, jobs,
  rendering, playback and evidence tools are Rust. Unsafe Rust is confined to
  that adapter's resource and length checked FFI module.
- The probe accepts BT.709/sRGB SDR primaries/transfer, assumes BT.709 for missing
  primaries/transfer, and reports that assumption. Unknown HD YUV matrices use
  BT.709; smaller unknown sources use BT.601. HDR/log and other gamuts fail
  explicitly. Full input/working/display/output color management remains R2/R7.
- Two GPU uploads and two explicit float readbacks per picture compare both
  precisions. FP16-to-F32 CPU conversion is slower on this ARM route. No zero-copy
  or real-time 4K claim is made. R2 must retain GPU textures for preview, pipeline
  export readback, choose precision from measurements, and qualify caches/seeks.
- The exporter is a working picture-only feasibility route with constant-rate
  timing and an RGBA8 output boundary. Sound muxing, VFR conformity, a delivery
  queue, source/revision-owned project jobs and professional formats remain open.
- Vulkan on this ARM64 machine is proven. Intel/AMD/NVIDIA, other GPU backends,
  hardware codecs, surface presentation, device xruns and two-hour sync remain
  explicit qualification work. The system FFmpeg enables GPL/version3 features;
  release packaging must record and satisfy that binary's distribution terms.

Implementation references: [FFmpeg decode](https://ffmpeg.org/doxygen/trunk/decode_video_8c-example.html),
[FFmpeg encode](https://ffmpeg.org/doxygen/trunk/encode_video_8c-example.html),
[wgpu 30](https://docs.rs/wgpu/30.0.1/wgpu/),
[CPAL callback timestamps](https://docs.rs/cpal/0.15.3/cpal/struct.OutputCallbackInfo.html).
This closes the measured ARM64 prototype slice of EB-004; R0 still needs EB-005–007.
