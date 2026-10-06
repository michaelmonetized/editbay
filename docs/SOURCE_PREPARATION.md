# Sound prepared before playback

Issue [#55](https://github.com/michaelmonetized/editbay/issues/55) follows canonical
PCM in [PR #54](https://github.com/michaelmonetized/editbay/pull/54).

The sound compiler bounds source time reached by a captured output interval. It
propagates inclusive enclosing intervals forward through node ranges, enabled
tracks and each piece of nested clip time maps. Reverse maps preserve both ends;
freezes and inactive paths contribute no source work. Fractional bounds round
outwards at each level. Planning uses the existing interval and operation limits,
checks cancellation and does not enumerate the duration's output samples.

The shared renderer expands those bounds by the maximum supported sinc radius,
clamps to captured source presentation and deduplicates original-channel source
requests. Source handles, storage handles and required disk bytes are checked
before decoding. The existing 16× supported slope and sinc constants define the
conservative margin; actual cache allocation still enforces the disk ceiling,
including complete decoder blocks beyond a requested boundary.

Each preparation step advances at most the PCM provider's native decode budget
and returns validated actual source progress. The supervised codec route publishes
no output mapping for preparation. Source progress, stale renderer ownership,
cancellation, codec death and pin accounting retain their normal checks.

Playback prepares all reachable source history from the selected start through
the sequence end before creating the device stream. Its callback queue, block
size and clock bounds are unchanged. Delivery uses the same plan and returns
progress between bounded source steps, then renders only the requested output.
Discarded composition pre-roll is removed. Delivery's parent independently
derives the required source total and rejects forged or regressing child progress.

Current local tests cover conservative bounds against every planned sample center
for nested/reverse/segmented/fractional paths and a two-source future reverse cut
with one decoder. After preparation, rendering the complete fixture backwards
matches normal evaluation without increasing decoded frames or stored bytes.
Actual-worker, real-media whole/range, native workflow and full workspace
qualification are pending. No sustained-clock, hardware, physical audibility or
release/adoption gate is closed by this dependency.
