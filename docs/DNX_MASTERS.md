# DNxHR HQX masters

The delivery panel and `export-profile` support `dnxhr_hqx_mov` and
`dnxhr_hqx_mxf`. Both use the existing native decoder, float compositor,
sound renderer, isolated delivery worker and exclusive publication path.
The encoder receives RGBA16, converts to limited-range BT.709 YUV422P10,
and explicitly selects DNxHR HQX. Sound is 48 kHz signed PCM24 mono/stereo.
Actual profile, pixel format, component depth, color, clocks, channels,
decoded counts and signal error are checked before publication.

Choose BT.709 output gamut and BT.709 output transfer. This is a real output
conversion; sRGB values are never relabeled as BT.709. The graph must be opaque;
add an explicit opaque background in the composition when needed. Integer PCM
rejects finite headroom outside [-1,1], rounds to the nearest 24-bit step and
saturates the positive full-scale endpoint to 8388607. Decoded PCM must remain
within one quantization step, with exact selected-range sample counts.

Supported progressive geometry is even and at least 256 by 120 pixels. Rates
are 24000/1001, 24, 25, 30000/1001, 30, 50, 60000/1001 and 60 frames/second.
MOV supports fractional selected-range sample phases. The initial MXF OP1a
profile requires integral 48000-samples-per-edit clocks: 30000/1001 and
60000/1001 are refused before a worker starts. The packaged MXF muxer's automatic
PCM rechunking would otherwise pad or shift the selected phase. Use MOV for
those clocks. Interlaced, arbitrary surround, HDR and alpha DNx profiles are
unavailable in this preset.

The focused `master_formats` tests create original analytic BT.709 PNG16/float
PCM fixtures, reopen the saved editable project, render on software Vulkan and
exercise real MOV/MXF export, native QC, cancellation and preservation. Missing
encoders are recorded as unavailable and fail the required case. This is
software qualification. Native decoding shares FFmpeg with encoding; it does
not establish independent codec conformance, external NLE compatibility,
desktop/device behavior or physical display/sound acceptance.

Native constraints were checked against the packaged FFmpeg n9.0.1
[DNx encoder](https://raw.githubusercontent.com/FFmpeg/FFmpeg/n9.0.1/libavcodec/dnxhdenc.c),
[MXF muxer](https://raw.githubusercontent.com/FFmpeg/FFmpeg/n9.0.1/libavformat/mxfenc.c)
and [PCM rechunker](https://raw.githubusercontent.com/FFmpeg/FFmpeg/n9.0.1/libavcodec/bsf/pcm_rechunk.c).
These are reference sources; their codec implementations were not copied.
