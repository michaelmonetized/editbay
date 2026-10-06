# Sustained native sound playback

Issue [#43](https://github.com/michaelmonetized/editbay/issues/43) follows the
exact-picture dependency, [PR #42](https://github.com/michaelmonetized/editbay/pull/42).
It measures the production sound worker over a saved linked sequence. A callback
receipt is backend evidence; physical speaker/display drift and independent-user
acceptance remain separate release gates.

## Clock and ownership

`ClockObservation` publishes device buffer start, submitted frames, host monotonic
time, backend timestamp epoch/time, latency and valid source end in one atomic
sequence. Readers make at most three attempts and retain their last coherent
observation on contention. Callback count derives from that same publication.
The existing source clock retains exact integer arithmetic, latency compensation,
monotonic position and submitted/valid-end caps.

The Linux ALSA backend may replace its timestamp origin when initial output
starts. One explicitly numbered epoch change is allowed within the first host
second. A second or later reset fails visibly; epochs are never joined into a
fabricated continuous timestamp. A backend reset while draining the tail also
fails instead of reporting completion. Source underflow, device failure and user
cancellation retain their existing distinct outcomes.

The callback only consumes prepared samples, reads backend timestamps and writes
fixed atomics. It allocates no application memory, takes no application locks,
and performs no source filesystem access or decode. Device discovery, codec work,
serialization and status observation stay outside it. This statement is about
EditBay's callback; it does not certify every internal allocation in CPAL/ALSA or
the external sound server.

After one backend second, the callback compares elapsed submitted frames with
the change in predicted backend presentation time. A difference over 20 ms
silences the current device buffer before consuming more source sound and latches
a visible continuity failure. The cleanup path preserves that original failure
when codec cancellation also returns an error. A real owned-device STOP/CONT
trial covers a recovered backend interruption, separately from source underflow.

Supervised device replies retain session/version/serial checks and now reject
missing, mixed or regressing callback observations, overlapping device intervals,
oversized callback buffers and submissions beyond the bounded tail allowance.
The protocol permits skipped polls without accepting altered data for the same
callback count. Observer data never permits stale work to advance another job.

The producer still uses 4096-frame blocks and at most 16384 prepared frames. The
new device request is 2048 frames, clamped to the supported range; at 44.1 kHz this
is 46.4 ms. The earlier 512-frame request was 11.6 ms and encountered a backend
reset during the first sustained candidate. Reported backend latency and actual
callback intervals are measured separately; requested buffering is not a measured
speaker latency. The maximum accepted callback remains the preparation capacity.
The current route prefers supported 48 kHz output with the reported default
channel count and sample format; otherwise it retains the default configuration.
The shared renderer performs exact-rate conversion. Actual device rate and sample
boundaries remain explicit in every playback status.

## Repeatable local qualification

Run the frozen packaged Rust lab with `LD_LIBRARY_PATH` cleared and `TMPDIR`
inside the project:

```
editbay-lab sustain-sound SOURCE SECONDS NEW_DIRECTORY
```

The command requires video and sound and a 3–7200 second duration. It imports
original media, authors up to 256 linked source intervals through ordinary
reversible commands, checks undo/redo, saves `Sustained.editbay`, and reopens the
same exact composition. A fractional frame endpoint rounds up to the next exact
sequence frame and is reported. Original media is read-only.

Actual supervised device playback starts at sample zero and must finish at its
exact declared sample end. Every 100 ms, `samples.jsonl` records coherent callback
status, prepared occupancy, combined lab/descendant RSS and host memory/CPU stall
counters. Memory keeps fixed summaries and at most 512 observed process IDs;
the trace is limited to 75000 records / 128 MiB. A limit, source change, device
error, missing cleanup or interrupted run cannot pass. The command's process
exit alone is not acceptance: inspect `qualification.json.qualified` and the
separate `two_hour_run_complete` field.

The predeclared clock gate is maximum absolute drift <=20 ms against both
backend and host estimates. Compute elapsed device frames minus the elapsed sum
of backend callback time and reported playback latency, relative to the first observed
callback after one backend second. No anchor crosses an epoch. Raw callback-time
variation and host-time variation are separate fields. Startup samples remain
in the trace; only the steady drift summary excludes that first second. Absolute
p50/p95 values are histogram upper bounds at 0.1 ms resolution, with explicit
200 ms overflow; maximum and final signed drift retain nanosecond calculations.

Additional gates are exact completion with no reported underflow/reset, prepared
occupancy <=16384, sampled combined RSS <=4 GiB, unchanged source/project hashes
and owned-child retirement. Host pressure counters record workload conditions;
they do not themselves identify a cause for a failure. RSS can count shared pages
twice and excludes external sound-server memory, unmapped page cache and
unreported driver/device allocations. Periodic observations do not capture every
callback or transient allocation peak. A short pass cannot close the two-hour gate.

## Current evidence

[Candidate and failure evidence](evidence/r2-sustained-clock/README.md) retains
the startup-epoch failure, a later 512-frame clock failure and a 2048-frame source
underflow during concurrent builds/native fault trials. The 2048-frame candidate
passes six-second camera/six-channel trials, a repeated 60-second AAC run, native
fault/retry regressions and local locked checks. Its separate baseline fails
after 174 seconds with accumulated clock error and source underflow. Supported
48 kHz output completes a three-minute run within 1.073 ms clock error. The final
continuity/error-preservation runtime passes native regressions and local checks;
its actual two-hour attempt fails after 1100.104 seconds with explicit backend
timing discontinuity, queued source sound and complete child retirement. Observed
backend/host maxima are 19.030482/25.783440 ms; the rejected callback is not
published as an accepted clock observation. Short passes do not erase sustained
failures or prove physical output timing.
