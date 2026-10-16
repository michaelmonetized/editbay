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

Mastering-display/content-light metadata and ICC profile handling are separate
pending cuts. Container color tags alone do not establish those fields or physical
HDR display accuracy. No desktop, audio-device or physical timing run is implied.
