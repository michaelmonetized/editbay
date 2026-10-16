# Retained source ICC color

Native ingest captures an embedded RGB ICC profile from the selected decoded video
stream. The complete bytes and SHA-256 live in `SourceStream.icc_profile`; missing
fields in existing documents default to `None`. Changing profile bytes or effective
profile presence within a stream is rejected during indexing.

The existing picture decoder, cache, worker and prepared store retain source samples
at their captured precision. The source operation normalizes integer pixels to float
and applies the retained profile through Little CMS 2 into the selected linear
BT.709, BT.2020 or Display P3 working gamut, before spatial sampling. The same float
graph continues through effects, scopes and delivery. Input profile application
does not also apply the source transfer or gamut tags. YUV matrix/range conversion
still requires explicit supported source tags; an RGB ICC does not describe a YUV
matrix. Untagged RGB images can use the captured ICC interpretation.

Encoded premultiplied input is unassociated before the ICC transfer curves. The
native transform returns straight float RGB and preserved alpha; the source shader
premultiplies once. Additive nonzero light with zero premultiplied alpha is rejected
rather than silently discarded. Float values remain float through the transform;
individual ICC curves and LUTs determine their supported domain.

Use **Source ICC interpretation** below the native authoring panes to choose the
captured source video stream and original profile path, assign or clear the profile.
Bounded file IO and native profile validation run on an owner-captured worker. A tab,
revision or activation change rejects its result. Save retains the interpretation;
Undo restores the prior source. No source file is rewritten.

The same typed command is available from the CLI:

```
editbay source-profile FILE SOURCE_ID STREAM_INDEX ICC_FILE
editbay source-profile FILE SOURCE_ID STREAM_INDEX none
```

Profile input must be a regular file no larger than 1 MiB, with complete RGB ICC
framing and matching content identity. Native color parsing must succeed before
the user route commits the assignment. Clearing uses the source color tags again.
Malformed, unsupported or ambiguous source interpretations fail visibly.

Little CMS 2 is an additional native dependency discovered with `pkg-config lcms2`.
Its MIT license is retained in `assets/native-licenses/lcms2-MIT.txt` and included by
the existing package license collector. The existing native dependency closure
collector includes the linked library. No Rust registry dependency is added.

Source profile capture and conversion are separate from output ICC embedding,
mastering/content-light metadata, general ICC conformance and physical display
qualification. Those require their own evidence. The original matrix/TRC and PNG16
fixtures use independent generation and analytic gamma/matrix expectations; the
Little CMS implementation is not its own numeric oracle.
