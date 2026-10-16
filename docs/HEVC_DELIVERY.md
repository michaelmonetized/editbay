# HEVC Main10 MP4

`hevc_main_mp4` is a separate explicit **8-bit Main SDR** choice. It uses
RGBA16 input to the same encoder conversion, then quantizes to YUV420P8.
Its receipt reports the precision and chroma loss. It accepts BT.709/sRGB
SDR, never silently substitutes for Main10 and carries no HDR qualification.

On the inspected packaged libx265, Main10's required pixel format is absent.
`delivery-formats` records Main10 as unavailable, the native UI disables its
destination action, and CLI delivery refuses it before a worker starts.
Historical executed failures at14d6496 and7f2721d remain failures. Current
Main10 export rows are unavailable, with no export pass. Its optional explicit
Main10 software cases stay ignored unless run on a matching packaged encoder;
ignored cases never count as delivered output. Main8 has its own real export
cases, numeric QC and independent format inspection.

`hevc_main10_mp4` in `export-profile` and the delivery panel uses the existing
native writer and supervised QC/publication route. RGBA16 reaches libx265 as
limited-range YUV420P10. The preset explicitly selects Main10, `hvc1`, CRF16,
no B pictures and at most two frame threads with no additional worker pool.
Sound uses the existing AAC route, 192 kb/s mono or 320 kb/s stereo at 44.1 or
48 kHz. Output geometry must be even and at least 16 by 16 pixels.

BT.709/sRGB and BT.2020 PQ/HLG output use the saved float color transform.
Static mastering/content-light metadata is explicitly unavailable in this
preset. Use the lossless16 master profile when those static tags are required.
HEVC requires an opaque composition and refuses sound outside full scale.
Encoded profile, component depth, pixel format, color, exact picture clocks,
channels, decoded counts and signal tolerances are checked before publication.
Picture RMS error must be at most 0.015 normalized units; AAC RMS error must
be at most 0.02, with at most 1023 final decoded padding samples.

`editbay delivery-formats` queries the packaged encoders, decoders, pixel formats
and container compatibility. Every result is `available`, `unavailable` or
`undetermined`, with a reason. Its qualification field explicitly says that
the query did not run a case. Registration never establishes codec/profile,
timing, color, channel, machine or device qualification.

The focused `craft_video_hevc_` cases use original analytic BT.709 bars/ramps
and indexed chirped tones whose selected source origin is distinguishable.
They exercise mono/stereo, a fractional selected
range, saved PQ/HLG color with undo/reopen, real native delivery/QC and
cancellation/no-overwrite preservation. HDR gray levels are also checked with
independent f64 reference equations. Execution receipts, rather than these
test definitions, establish the tested cases. There is no device, desktop,
physical HDR display or independent HEVC picture-conformance claim.

This adapter uses the existing packaged FFmpeg and its libx265 interface; it
does not copy their implementations. The inspected host packages declare
FFmpeg 9.0.1 GPL-3.0-only and x265 4.3 GPL-2.0-or-later. The existing package
collector retains native package provenance, installed dependency licenses
and shared license texts. No new Rust registry dependency is introduced.
