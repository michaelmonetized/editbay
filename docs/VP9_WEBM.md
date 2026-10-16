# VP9 Profile 2 WebM

`vp9_profile2_webm` uses the same native writer, RGBA16 boundary, float graph,
supervised delivery and atomic publication as other profiles. Video is opaque,
limited-range YUV420P10, VP9 Profile 2, CRF16 with no lookahead and two threads.
Sound is 48 kHz mono/stereo libopus with a bounded native FIFO. Static HDR
metadata is unavailable; saved BT.709/sRGB or BT.2020 PQ/HLG color is supported.

This initial preset requires integral millisecond frame intervals, including
25/50 frames per second. WebM's native muxer uses a millisecond clock; 24,
30000/1001 and other nonintegral millisecond intervals are explicitly refused
before starting a worker. The file never silently rounds an exact requested
picture grid. The output preserves picture zero while Opus pre-skip and final
discard remain part of the codec/container timing path.

Native QC requires the actual codec/profile/pixel depth/color, picture clock,
channel layout and sample rate. It checks every decoded picture timestamp,
picture RMS error <=0.015, sound RMS error <=0.02 and at most 959 final Opus
padding samples. Focused analytic fixtures independently require best sound
lag zero, exact selected source sample endpoints and correct PQ/HLG gray codes.
Cancellation, source preservation, save/reopen and exclusive destination
publication use the existing routes.

Test definitions and codec registration are not execution receipts. Missing
encoders are unavailable and fail required native cases. Encode/decode share
FFmpeg; no complete independent VP9/WebM conformance, external player,
desktop/audio device or physical display/sound behavior is established here.

The adapter references the packaged FFmpeg n9.0.1
[libvpx interface](https://raw.githubusercontent.com/FFmpeg/FFmpeg/n9.0.1/libavcodec/libvpxenc.c),
[libopus interface](https://raw.githubusercontent.com/FFmpeg/FFmpeg/n9.0.1/libavcodec/libopusenc.c)
and [WebM muxer](https://raw.githubusercontent.com/FFmpeg/FFmpeg/n9.0.1/libavformat/matroskaenc.c).
Their implementations are not copied. Existing package license collection and
native library closure apply; no Rust registry dependency is added.
