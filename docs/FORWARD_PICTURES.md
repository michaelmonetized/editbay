# Continue decoding across short forward gaps

The retained Rust picture cache can advance over at most eight indexed pictures
without flushing its native decoder. This prevents a small forward jump from
automatically restarting at an earlier keyframe. The exact next picture retains
its normal sequential route; backward and larger jumps retain the exact seek
route. Cache hits do not change the decoder's next ordinal.

Each skipped picture is decoded without RGBA conversion or an output-plane
allocation. Its actual presentation timestamp must equal the corresponding
captured index entry. The requested picture then follows the normal geometry,
metadata, exact-tick and source-ownership checks. No nearby picture substitutes
for missing content. A failed advance drops the decoder and the unpublished
output allocation; a later request starts with a fresh decoder.

This is the same `PictureCache` used inside the packaged Rust picture worker.
It adds no source, decoder, cache, mapped-plane or live-payload capacity. Consumer
pins remain charged through eviction and clear; private owner/version/generation,
source verification, cancellation and supervised process deadlines are unchanged.
Native decode still checks cancellation between packets and frames. The eight
picture bound limits extra decode work, not its elapsed time on arbitrary codecs.

`sequential_decodes`, `forward_decodes` and `seeks` count requests entering each
route. `skipped_pictures` counts intermediate decoded timestamps validated during
forward advancement, including progress before a later failure. It is different
from the native viewer's `skipped_frames`, which currently counts gaps between
accepted worker pictures. That viewer counter is not exhaustive display-completion
evidence. IPC hit qualification checks that none of the decode counters move on hits.

Tests use actual variable-rate H.264 with B-frames and independent FFmpeg RGBA
output. They cover the eight-picture boundary, the next larger jump, reverse/EOF,
hit state, decoder eviction, forged intermediate timing, cancellation and held
allocation cleanup. Real media qualification also sends uncached requests through
the packaged app/CLI workers and checks every returned hash/color/alpha/tick,
route counter, process retirement and empty output-handle accounting.

`editbay-lab native-continuous APP SAVED_PROJECT NEW_DIRECTORY` observes an entire
saved sequence, capped at ten minutes, using the actual native sound clock. It
records request-to-GPU-completion, displayed pictures at sound end, sampled
process RSS/high water and source/project preservation. Shared pages can appear
in more than one process's RSS; GPU allocation counters remain separate. It
reaps the application and every observed owned child. Short source trials do not
establish physical audibility, two-hour drift or the 1080p/4K hardware matrix.

Issue [#36](https://github.com/michaelmonetized/editbay/issues/36) is published in
[PR #39](https://github.com/michaelmonetized/editbay/pull/39), following
[PR #37](https://github.com/michaelmonetized/editbay/pull/37). Dense random edits,
prepared proxy/render caches and hardware decode remain further R2 work; none of
the existing frame, seek, memory or cancellation gates is relaxed.
