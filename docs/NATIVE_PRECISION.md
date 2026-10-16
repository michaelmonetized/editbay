# Native picture precision

New imports capture the actual decoded storage as `editbay.pixel_encoding` in
the stream interpretation: RGBA8, little-endian RGBA16, or little-endian float
RGBA. A change of precision within a stream is rejected during indexing. Color
primaries, transfer, matrix, range and alpha interpretation remain separate.

The existing native decoder, cache, shared worker planes and prepared packs carry
that encoding. Allocations and disk plans charge the actual bytes per pixel.
Worker and pack headers must agree with the captured interpretation. Missing
encoding in older captures and headers defaults to RGBA8; reimport legacy media
to capture its original precision. Existing explicit RGBA8 reader APIs retain
their byte contract. Precision readers use the same native decoder and aligned
conversion storage carried from PR69.

The existing float compositor uploads high-depth components as normalized float
RGBA. Integer low bits survive conversion; float inputs retain finite negative
values and headroom. Working half precision remains an explicit project choice
and can reduce precision. Full precision is the default.

`Lossless 16-bit MOV` / CLI `lossless16_mov` writes straight RGBA16 PNG pictures
with original-layout float PCM, exact rational frame/sample clocks and Rec.709
primaries/sRGB transfer tags. Delivery quantizes once at the output boundary,
counts clipped RGB components, and verifies the encoded file by decoding native
16-bit pixels and comparing exact hashes against the rendered output bytes.
Cancellation, exclusive destination ownership and atomic publication use the
existing delivery route. This SDR format does not advertise HDR or ICC delivery.

Focused media fixtures use original hand-encoded PNG16 ramps and native writers.
The pipeline fixture covers actual worker transport, prepared storage, independent
sRGB math, retained low bits/alpha, save/reopen, stale receipts, budget rejection,
range delivery, cancellation after rendering and existing-file preservation.
Runtime receipts identify their exact source and binary; source presence alone
does not qualify these routes. FFmpeg-to-FFmpeg comparison is not independent
codec conformance. Desktop, sound-device, physical display and human checks are
separate from these software checks. ICC/HDR and additional delivery families
remain separate implementation cuts.
