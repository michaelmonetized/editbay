# Native sequence playback

Play prepares the saved sequence graph on a Rust worker and sends bounded PCM
to the default output device. Pause retains the exact device-rate sample boundary.
Resume uses that boundary; a frame seek replaces it. Editing, switching documents,
leaving the workspace or cancelling the viewer cancels the captured playback.
Play after completion starts again at zero. A failed source, device or preparation
queue stops with a visible error; Play creates a fresh lifetime.

## Preparation and device work

`StreamingPlayback` captures an immutable project and its document version.
Its supervised packaged `--sound-device-worker` owns device discovery, graph
compilation, the packaged `--pcm-worker` child, shared `SoundRenderer`, monitor
routing and stream teardown. The UI requests cancellation, copies the latest
validated status and joins only finished supervisors. It never waits for device,
codec or filesystem work. See [device isolation](SOUND_DEVICE_WORKER.md) for
ownership, response deadlines and failure recovery.

The producer renders 4,096-frame blocks. A preallocated single-producer,
single-consumer queue retains at most 16,384 device frames regardless of sequence
duration: 128 KiB of sample storage for stereo. Raw PCM cache, process mappings
and rendered output retain their existing independent budgets and pin lifetimes.
The producer primes the queue before starting the stream, then checks source
identity and child health even while the queue is full.

The data callback fills the device-owned slice from prepared samples, converts
sample format, and publishes integer cursors/timestamps. It does not allocate,
deallocate, lock, decode, resample or access files. Fixed atomic publication and
at most three read attempts bound clock observation. Cancellation uses the native
lock-free atomic flag; the C adapter requires lock-free integer atomics at compile
time. CPAL/backend internals are outside this callback contract.

The clock counts every device frame, including final silence padding, and caps
source time at the exclusive sequence end. This allows backend latency longer
than one buffer without hanging at the last sample. It estimates presentation
using the backend timestamp and integer arithmetic; it does not measure speakers.
Underrun latches a failure, silences remaining output and cancels the codec child.
Late preparation cannot revive that queue or silently skip missing source sound.

## Listening route

The actual default device rate is explicit in the UI. Shared sound evaluation
resamples at that rate. Only reachable audio leaves determine source channel
identities; incompatible layouts fail rather than implicitly mixing different
speaker identities. A sequence without audio plays explicit stereo silence.

Stereo monitor supports a two-channel output. Mono center goes to both speakers
at unity. Multichannel FL/FR retain unity; center and matching-side rear/side
channels contribute at sqrt(0.5); LFE is omitted. The off-callback matrix clips
monitor output to [-1, 1] and counts clipped samples visibly. Original channels
requires a matching mono-center or stereo-FL/FR device. Unsupported routes fail
visibly. This is a listening route: original graph samples and delivery channels
are unchanged. Device selection and broader calibrated channel routes remain open.

## Pictures and ownership

The audio sample clock chooses the requested video frame using rational integer
time. The existing bounded picture worker coalesces requests. During playback the
viewer accepts the newest completed non-backward picture for its current owner,
counts skipped pictures and keeps sound running. Pause and seek restore strict
request matching. Old document or sequence results cannot enter a new viewer.
Normal preview retains GPU textures without reading pictures back to the CPU.

## Qualification contract

`editbay-lab stream-sound SOURCE [full|cancel|kill|underrun]` exercises the actual
device through this production worker. `native-playback APP SOURCE NEW_DIRECTORY
[full|half]` exercises window controls, exact pause/resume, seek, codec death,
underrun, edit cancellation, save and independent recovery. Source files remain
read-only. Device submission is separate from physical audibility and hardware
drift.

Budgets set before qualification: at most 16,384 prepared frames, monotonic
sample time capped at valid submitted sound, exact final sample, no surviving
owned codec process and cancellation/failure retirement <=2 s. Retain the parent
native input p95 <=50 ms, cached draw p95 <=250 ms, sound preparation p95 <=5 ms
and unity rendering p95 <=20 ms gates. Failed candidates remain evidence.
The full R2 1080p/4K drop, seek, two-hour drift and hardware matrix gates in
`RELEASE_CRITERIA.md` stay unchanged and require their own actual measurements.
