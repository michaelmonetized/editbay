# Supervised sound devices

Production playback runs device discovery, CPAL/ALSA/PipeWire initialization,
stream creation, callbacks and teardown inside the packaged
`--sound-device-worker` endpoint. An audio backend changing process-wide limits,
crashing or hanging can fail that playback without terminating the editor.
The CLI, lab and native application ship the endpoint in the same Rust binary.

`StreamingPlayback` owns an asynchronous supervisor and one latest status. It
captures an immutable project, composition, exact frame/sample start and explicit
monitor route. The child retains the existing `LocalPlayback`, `SoundRenderer`,
`PcmWorker`, sample clock and 16,384-frame prepared queue. Its codec subprocess
still owns the bounded native PCM cache, source descriptors, pins and cancellation.
There is no second rendering implementation or full-sequence PCM allocation.

The device callback still reads prepared samples and publishes fixed atomic clock
fields. It does not allocate, lock, use IPC/filesystem access or decode. Status
serialization and socket traffic run on the child's control thread. The editor's
supervisor samples that status every eight milliseconds; the UI copies one retained
status through a short pointer lock. It does not wait for IPC, device access,
preparation or process teardown. This adds sampled observation delay; it does not
claim direct shared-memory clock access or physical speaker timing.

## Ownership and failure

A fresh random session, captured document version and increasing request serial
bind every operation and reply. The first operation starts one playback. Subsequent
operations only inspect that same lifetime. Duplicate starts, old or foreign
requests, unknown fields and descriptors are rejected. The existing private native
transport limits messages to 8 MiB and retained stderr to 8 KiB.

The parent independently checks document identity, route, unchanged device/rate/
format and codec PID, exact rational start/end sample boundaries, monotonic clock
and callback counters, buffer occupancy and failure/completion consistency. A
child cannot substitute a different playback or declare success before the exact
end. Only the supervisor supplies its device process ID.

A reply has a 500 ms deadline. Preparation has a 30-second deadline; a playing
stream with no new callbacks for a second fails. Cancel interrupts pending waits
and wakes the supervisor. Closing the private connection tells a healthy child
to stop. Process supervision kills and reaps a child that has not exited after
100 ms. The existing bounded socket write can add at most 250 ms. All this work
runs off the UI thread, with a two-second observed retirement acceptance gate.

The device and PCM children each use a parent-death signal and verify parent
identity at spawn. Abrupt controller/device death terminates descendants; Linux's
reaper collects orphaned exit statuses. The lab uses an explicit subreaper and
waits only for the recorded descendants to make that cleanup observable. The
production editor does not install a global child reaper that could steal other
workers' exit statuses.

A failed or stalled backend leaves a visible preview error. After retirement,
Play creates a new owner, device process and codec process. Pause retains the last
observed exact integer sample for resume. Seeking and document changes retain the
existing captured-owner checks and retire obsolete playback. Source and export
channels are unchanged by the explicit monitoring route.

## Qualification

Run all commands with project-local `TMPDIR`. Native timing uses optimized release
executables with recorded source and binary hashes; an unoptimized six-channel
resampling trial can underrun and is not playback qualification.

```sh
editbay-lab stream-sound SOURCE full
editbay-lab stream-sound SOURCE kill-device
editbay-lab stream-sound SOURCE stall-device
editbay-lab stream-sound SOURCE cancel-stalled-device
editbay-lab stream-sound SOURCE prepare-cancel
editbay-lab device-protocol PROJECT COMPOSITION
editbay-lab device-parent PROJECT COMPOSITION NEW_DIRECTORY
editbay-lab native-device APP_BINARY SAVED_SOURCE_PROJECT NEW_DIRECTORY
```

The existing `cancel`, `kill` and `underrun` stream modes still exercise ordinary
cancellation, PCM worker death and PCM starvation. `native-device` kills/stalls
real active device workers, checks visible failure and process retirement, retries
real callbacks, cancels a stalled worker, then authors linked cuts, undoes/redoes,
saves, recovers, exports and reopens the native project. It independently decodes
the exported master. Unit tests also reject substituted/malformed reply fields and
verify exact fractional-rate conversion and retained terminal failure.

Physical audibility, unplugged hardware, two-hour device drift, sustained 1080p/4K
playback on all supported hardware and full R2/R3 acceptance remain separate gates.

[Recorded native/process evidence](evidence/r2-sound-device/README.md) passes the
scoped device-containment gates. It retains a camera planning timing miss and
keeps full sound-performance, hardware and release acceptance open.
