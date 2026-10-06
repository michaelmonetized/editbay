# Native sound backend contract

Issue [#47](https://github.com/michaelmonetized/editbay/issues/47) follows
[PR #46](https://github.com/michaelmonetized/editbay/pull/46). The earlier
[two-hour attempt](evidence/r2-sustained-clock/README.md) failed after 1100.104
seconds. A dependency upgrade is a candidate fix, not sustained acceptance.

## Device buffers and faults

The workspace pins a local CPAL 0.18.2 copy with one documented ALSA snapshot fix;
see `vendor/cpal/EDITBAY.md`. Its ALSA fixed-buffer request sets the period to
approximately the requested frame count, subject to hardware negotiation. The
previous 0.15.3 implementation requested a period of one quarter that count;
`Fixed(2048)` produced 512-frame callbacks in the recorded device workload.
See the [upstream implementation](https://github.com/RustAudio/cpal/blob/v0.18.2/src/host/alsa/mod.rs)
and [release](https://github.com/RustAudio/cpal/releases/tag/v0.18.2).

The production route keeps the default output device, default channel count and
sample format, and prefers supported 48 kHz output. It requests 2048 callback
frames clamped to the supported range. The requested and reported sizes are
separate optional fields in each version-owned device receipt. A missing backend
size report is not replaced with the request. Every actual callback independently
rejects empty, misaligned or more than 16384 frames before consuming source sound.
The producer retains its 4096-frame blocks and 16384-frame prepared capacity.

Negative latency and invalid timestamps remain errors through checked timestamp
arithmetic. The existing single startup epoch and 20 ms continuity guard remain.
An actual backend xrun now produces a distinct visible error and cancels source
work. Backend faults and malformed callbacks can interrupt the final latency
drain; they cannot overwrite an earlier failure or turn it into successful end.
No failure skips source samples to catch up.

The application callback only accesses fixed state, atomics and preallocated
slices. Device discovery, stream setup, source rendering, serialization and
filesystem operations remain outside it. This does not certify allocation or
locking inside the platform backend or external sound server.

The ALSA adapter obtains delay from the same status container as its timestamp.
The earlier separate availability/delay query remains a readiness check. Pairing
its older delay with a later status time could include a preemption gap twice.
A negative snapshot delay is now an xrun. This follows the
[ALSA status contract](https://www.alsa-project.org/alsa-doc/alsa-lib/group___p_c_m___status.html);
it does not establish the cause of any previous recorded failure.

The first rejected continuity check has its own fixed atomic receipt: submitted
buffer start, host/backend time, reported latency and signed drift. It never
advances accepted samples. Later callbacks silence immediately after cancellation,
and cannot replace that first rejection. Private device replies reject nonterminal,
changed, undersized-drift or incorrectly positioned rejection records.

## Scheduling evidence

CPAL's upstream `realtime-dbus` support may request real-time scheduling during
thread startup. Its ALSA adapter excludes plugin routes where promotion can cause
priority inversion, including server-backed I/O plugins. EditBay does not force
priority onto those threads, change system limits or substitute a null device.
Enabling this feature is not proof that promotion succeeded.

The Rust `sustain-sound` qualifier records the owned device process's thread names,
scheduling policy, real-time priority and nice values from
[Linux procfs](https://docs.kernel.org/filesystems/proc.html). It samples once per
second, retains at most 256 threads per sample and 1024 distinct records, and
handles threads exiting during observation. Callback size minima/maxima describe
only the coherent clock observations sampled every 100 ms. Neither sampling
method claims to observe every transient change.

## Qualification

The [sustained playback](SUSTAINED_PLAYBACK.md) command and all its existing gates
remain: exact sample endpoint, maximum 20 ms backend and host clock error, at most
16384 prepared frames, sampled combined RSS at most 4 GiB, unchanged source/project
hashes and complete owned-child retirement. Read both `qualified` and
`two_hour_run_complete`; process exit alone does not pass the gate.

The unpatched upstream candidate `faf9930` passes six-second camera and six-channel
runs at 2048 requested/reported/observed callback frames. Maximum sampled host drift
is 0.067475/0.072669 ms. Its three-minute trial stops after 91.471896 seconds with
backend continuity failure while 14336 source frames remain prepared; all children
retire and original hashes remain unchanged. Accepted backend/host maxima are
1.667293/2.660514 ms; the rejected callback was not captured in that candidate.
The actual 300 ms owned-device STOP/CONT test passes its visible-failure gate.
This trial ran amid substantial host memory pressure and is retained as a failure.

The snapshot fix, rejection receipt and full workspace checks are in progress.
Physical audibility, speaker/display timing, other hardware, client approval and
independent-user acceptance remain separate open gates.
