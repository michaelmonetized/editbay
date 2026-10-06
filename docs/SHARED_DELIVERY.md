# Shared picture and sound delivery

Issue [#26](https://github.com/michaelmonetized/editbay/issues/26) adds a full-sequence
master from the same captured `EvaluationSnapshot`, `GraphRenderer`, `SoundSnapshot`
and `SoundRenderer` used by preview. The app's **Export…** opens the native save
chooser. One background export can run at a time; eight recent results retain
progress, failures, cancellation and **Retry current version**. Closing or changing
the owning document cancels before mutation. Undo, redo, background commands and
save follow the same rule. Closing the app waits for accepted cancellation to
retire. A job already entering publication finishes its captured revision, which
is displayed on its receipt.

```
editbay export PROJECT COMPOSITION_ID NEW_MOV [SAMPLE_RATE]
```

The native control uses 48 kHz. The CLI accepts an explicit supported rate. Both
refuse existing destinations, including dangling symlinks. Neither changes source
media or the project. CLI SIGINT/SIGTERM requests cancellation; SIGKILL is contained
by the worker's parent-death signal and anonymous output lifetime.

## First master profile

- Full composition duration at its exact rational picture rate; first picture 0.
- MOV with lossless PNG RGBA8, straight alpha, Rec.709 primaries, sRGB transfer,
  RGB matrix and full range. The document output must use that gamut/transfer.
- Original declared channels as float32 PCM, retaining finite sound headroom.
  Mono, stereo, quad, rear/side 5.0/5.1 and 7.1 layouts are supported; unknown or
  reordered channel declarations fail visibly. Silent compositions use stereo.
- Sound ends at `ceil(pictures × rate × fps_denominator / fps_numerator)`.
  Container track clocks retain exact picture/sample timestamps. No padding to
  a guessed millisecond duration or monitor downmix enters the master.
- Output conversion uses the graph's explicit output boundary. Quantization to
  eight bits is intentional; RGB outside that range is counted and reported.
  Transparent RGB is canonicalized to zero. This is an SDR eight-bit master,
  not a high-precision/HDR delivery claim.

The native codec tests compare every decoded RGBA byte, including alpha, and every
float sample against actual mux inputs at fractional frame rates and 44.1/48 kHz.
PNG stores the frame's sRGB declaration in its own headers; setting only codec
context fields was insufficient. See the upstream [PNG encoder](https://github.com/FFmpeg/FFmpeg/blob/master/libavcodec/pngenc.c)
and the retained failed format candidates in the qualification evidence. The
profile is deliberately specific; no broad player, broadcaster or NLE compatibility
claim follows from a native FFmpeg round trip.

## Supervision and publication

`editbay-delivery` owns the controller, typed private protocol and worker session.
The packaged app, CLI and lab host `--delivery-worker`. A private Unix socket
transfers a single anonymous output descriptor. Requests are bounded to 8 MiB,
stderr to 8 KiB, and each request waits at most 120 seconds. The worker handles one
captured document/job and strictly increasing serials. Unknown fields, replayed
serials, foreign owners, unexpected descriptors, invalid progress and malformed
receipts fail. Dropping supervision closes the socket, allows 100 ms for retirement,
then kills and reaps stalled work.

Each render step produces one picture and PCM blocks of at most 4096 frames.
Existing picture, GPU and PCM budgets remain enforced. The worker synchronizes
its finished file, verifies retained sources, decodes every output picture/sample,
checks timestamps/counts/layout/color and compares SHA-256 hashes with the graph's
actual codec inputs. It rechecks sources before returning the receipt. The parent
reaps the child and verifies the file again before publication.

The file uses Linux `O_TMPFILE` in the chosen destination directory. It has no name
while rendering or checking. Descriptor-based verification avoids replacement of
a temporary pathname. A single atomic cancellation/publication decision determines
whether the final link may be created. Linking refuses an existing filename and
the directory is synchronized afterward. A directory-sync failure explicitly says
the master was published but durability could not be confirmed. Before publication,
normal failures and process death release anonymous storage without orphan files.
Filesystems without anonymous-file support fail visibly; there is no weaker
named-file fallback. See [Linux open(2)](https://man7.org/linux/man-pages/man2/open.2.html).

Document export guards share that same atomic decision and cancel before the core
editor's new revision is assigned. UI polling reports the outcome; it is not the
ownership barrier. Controller threads, filesystem work, GPU creation, decode,
render, verification and process teardown stay off the UI thread. Progress uses
one replaceable mailbox and results use a single bounded channel.

## Qualification and remaining scope

The lab provides `shared-delivery`, `native-delivery`, `delivery-protocol` and
`delivery-interrupts`; `--help` lists arguments. Evidence distinguishes synthetic
multichannel media from real camera media and software-injected native input from
physical-device measurements. Every output reopens; sources retain their hashes.
The [qualified camera/multichannel results](evidence/r2-shared-delivery/README.md)
include native export, recovery equality, process death and unchanged inputs.

This first profile does not close all R2/R3/R7/R11 gates. Arbitrary ranges, multiple
queued jobs, delivery presets, HDR/high precision, broadcast/OTT formats, hardware
encoding, long 1080p/4K workloads, other GPUs, independent-user proof and client
acceptance remain open. Very large files can exceed the per-request verification
deadline and fail without publication; they are not qualified by short fixtures.
