# Absolute picture color

`ColorConfiguration.hdr` saves reference-white and HLG display-peak luminance in
nits. Older documents default to 100-nit white and a 1000-nit HLG display. Values
are validated by the shared document commands, undo and save/reopen paths. Linear
working RGB 1.0 represents reference white. No tone map is implicit.

The existing native source operation accepts PQ (transfer 16) and HLG (18).
The same float graph performs PQ EOTF/encoding and HLG inverse OETF/OOTF/encoding.
HLG uses the declared source/output primaries for luminance and the saved display
peak for system gamma. RGB primaries continue through the existing linear gamut
matrix. Alpha is handled as straight source data and premultiplied working data.

Lossless16 MOV accepts explicit Rec.709, BT.2020 or Display P3 primaries and linear,
sRGB, BT.709, PQ or HLG transfer tags. Its native writer declares the already
converted output color, and native QC checks the actual container tags and exact
decoded 16-bit pixel hashes. Legacy 8-bit/H.264/ProRes profiles retain their SDR
contract and reject unsupported output color before encoding. Delivery receipts
retain the actual declared color. Missing color in older receipts defaults to
Rec.709/sRGB.

The existing `editbay apply FILE COMMAND_GROUP_JSON` route accepts a `set_color`
command with the complete configuration and exact expected document version.
For BT.2100 output, use BT.2020 working/output primaries, `pq` or `hlg` output
transfer, an explicit reference white such as 203 nits and the intended HLG peak.
The existing `lossless16_mov` delivery then uses that captured configuration.
Native panel field integration follows the parent UI merge; the current SDR
display path does not certify an HDR monitor.

Focused software fixtures write actual native color-tagged 16-bit sources and
deliver range masters through the packaged worker on CPU Vulkan. Independent f64
PQ and HLG formulas compare working light, actual program-scope nits and retained
output codes. The separate float fixture writes original FLOAT EXR bytes directly
and checks signed/headroom samples without an encoder oracle. These checks need
exact-source compiler/runtime receipts before being treated as passes.

`SourceStream.hdr_metadata` retains complete static mastering-display/content-light
declarations from actual native frame/container side data. Effective changes during
the stream are rejected. Chromaticities use integer units of 1/50000 and mastering
luminance uses 1/10000 nit, matching MOV `mdcv` delivery tags; native rational values
are normalized to those units. Content-light values are integer nits. Missing fields
in existing projects and receipts default to absent declarations.

`ColorConfiguration.hdr.metadata` declares the output metadata independently of
source tags. Shared SetColor commands, Undo and persistence retain it. High-depth
PQ/HLG delivery writes real `mdcv` and `clli` container data, and native QC compares
the requested values with the delivered file. Original sources are preserved.
These are saved declarations, not inferred or measured content/display luminance.
Unsupported partial mastering data, invalid values or changed source metadata fail
explicitly; general HDR metadata and codec-family conformance remain unqualified.

The integer layout follows the native muxer's [pinned MOV implementation](https://raw.githubusercontent.com/FFmpeg/FFmpeg/n9.0.1/libavformat/movenc.c).
Independent output inspection reads the actual MOV boxes and checks their values,
color tags, clock and 16-bit samples without running FFmpeg or the product binary.
Native UI field integration and output ICC embedding remain separate work. Container
tags do not establish physical HDR display accuracy. No desktop, audio-device or
physical timing run is implied.
