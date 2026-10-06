# CPAL 0.18.2 source

Copied from the crates.io CPAL 0.18.2 package. The original Apache-2.0 license,
manifest and Cargo VCS identity are retained. Cargo excludes this dependency from
EditBay's workspace membership; the pinned patch is shared by all app and worker
binaries.

Linux ALSA: `poll_for_period` obtains playback delay from the same
`snd_pcm_status` container as the callback timestamp. Upstream obtains delay from
an earlier `snd_pcm_avail_delay` call and combines it with a later status timestamp.
Thread preemption between those calls can add elapsed time without subtracting
consumed frames. The earlier availability call still detects readiness and errors.
A negative status delay is an xrun rather than a silently clamped zero.

Contract: https://www.alsa-project.org/alsa-doc/alsa-lib/group___p_c_m___status.html
Upstream: https://github.com/RustAudio/cpal/tree/v0.18.2

This fixes snapshot pairing; it does not prove that any recorded device failure
was caused by that race. Qualification retains original and patched candidates.

ALSA output also requires every consumed application frame to be submitted.
Valid partial writes continue from the unwritten suffix. Zero progress, invalid
write counts, EAGAIN, EPIPE and suspended writes fail visibly instead of dropping
a consumed period or spinning forever. Input behavior is unchanged.

Native PipeWire output sets `pw_buffer.size` in frames and includes queued and
resampler-buffered frames in presentation time. Graph delay uses its full rational
rate; queued frames use the negotiated stream rate. Checked integer arithmetic
rejects invalid timestamps and overflow. The callback timestamp comes from the
same monotonic clock at invocation, preserving absolute graph presentation time.
Before the first valid graph clock, at most one second of equilibrium is queued
without consuming application samples. A missing clock after startup fails.
The narrow raw-buffer guard owns dequeue/return, validates mapped geometry and
reports submission failure. No application-side unsafe code is introduced.

Contracts: https://docs.pipewire.org/structpw__time.html and
https://docs.pipewire.org/structpw__buffer.html.
The `output` module unit tests run with this manifest and the `pipewire` and
`realtime-dbus` features. Its generated test-only Cargo.lock is ignored and
retained with qualification artifacts; production uses the workspace lock.

Original crates.io package SHA-256:
`6f02e8d0327b42d3e2e4ab2119af397344eb9fc54a34bf0ddeaa1277af8681f1`.
Original Git commit: `e1612d5d98152f8dc2a62e1b51ef7cbf4f7f26b7`.
