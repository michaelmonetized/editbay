# CPAL 0.18.2 source

Copied from the crates.io CPAL 0.18.2 package. The original Apache-2.0 license,
manifest and Cargo VCS identity are retained. Cargo excludes this dependency from
EditBay's workspace membership; the pinned patch is shared by all app and worker
binaries.

One Linux ALSA change: `poll_for_period` obtains playback delay from the same
`snd_pcm_status` container as the callback timestamp. Upstream obtains delay from
an earlier `snd_pcm_avail_delay` call and combines it with a later status timestamp.
Thread preemption between those calls can add elapsed time without subtracting
consumed frames. The earlier availability call still detects readiness and errors.
A negative status delay is an xrun rather than a silently clamped zero.

Contract: https://www.alsa-project.org/alsa-doc/alsa-lib/group___p_c_m___status.html
Upstream: https://github.com/RustAudio/cpal/tree/v0.18.2

This fixes snapshot pairing; it does not prove that any recorded device failure
was caused by that race. Qualification retains original and patched candidates.

Original crates.io package SHA-256:
`6f02e8d0327b42d3e2e4ab2119af397344eb9fc54a34bf0ddeaa1277af8681f1`.
Original Git commit: `e1612d5d98152f8dc2a62e1b51ef7cbf4f7f26b7`.
