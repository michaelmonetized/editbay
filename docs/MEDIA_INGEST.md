# Native media ingest

The native workspace imports selected picture and sound streams into the shared
document. Choose **Import media**, select a local file in the native portal,
review the actual codec/geometry/rate/channel metadata, and choose **Add source**.
Import is one ordinary undo group. Save and recovery preserve its asset checksum,
stream indices, timing and channel order. An imported source is available for the
following composition engine; importing alone does not create timeline clips.

The CLI uses the same source adapter and document commands:

```sh
editbay probe-media /path/to/camera.mp4
editbay ingest /path/to/cut.editbay /path/to/camera.mp4 0,1
editbay decode-frame /path/to/camera.mp4 1 1200
```

Stream indices come from the probe, not an assumed video-first ordering.
`decode-frame` takes an exact indexed timestamp in the original video time base
and returns a receipt for actual full-range RGBA8 pixels. It does not write or
replace the source. CLI ingest acknowledges only after checked durable save.
Native import remains editable/unsaved until the normal Save action.
MCP media import/preview/export are still unavailable; its existing command and
frame-plan tools remain the documented scoped contract.

## Source and timing ownership

`SourceFile` opens a canonical regular file without following a replacement
symlink or blocking on a FIFO. It retains a read-only descriptor, original device/
inode/size/change times and a complete SHA-256. Native custom AVIO uses positioned
reads on a close-on-exec descriptor copy. Decode and seek cannot silently reopen
a replacement pathname. Before publication, both pathname identity and all source
bytes are checked again. A changed or missing source fails visibly.

Probe reports every container stream, including unsupported data/subtitle/
attachment streams and missing decoders. Only installed picture/sound decoders
can be selected. Source reel/timecode metadata, sample aspect, display rotation,
source color/alpha tags and channel names remain document data. Unknown channel
order is recorded as U0/U1/etc.; a guessed layout is never presented as declared.

Picture ingest decodes every presentation timestamp into a strict ordered VFR
index. Repeated/backward/missing timestamps and mid-stream color/alpha changes
require an explicit interpretation and currently fail. The exclusive last-frame
boundary uses decoded duration, then declared stream duration, then a recorded
nominal last-frame-duration assumption. Nominal rate never replaces the index.
Unknown source alpha is visibly interpreted as straight and recorded in metadata.
Random seek flushes decoder/resampler state and decodes forward to the exact
indexed picture, including after EOF and backward across delayed-codec frames.

Sound retains the original sample rate and channel order as interleaved finite
float samples. Decoded duration uses sample boundaries. Packet-time-base setup
preserves AAC skip/priming timestamps; declared presentation bounds remove codec
preroll/padding. Timestamp quantization is normalized only within half one source
tick. Real gaps/overlap remain errors. No silent resample, stereo downmix or sample
clipping is used by native ingest.

## Workers and budgets

The packaged native executable runs its own Rust `--media-worker` mode in an
isolated child process before UI initialization. No companion executable or
inference subprocess is required. Private typed JSON pipes are limited to 8 MiB
per message; requests, source work and results have bounded queues. Codec stderr
has a continuously drained 8 KiB tail. The adapter rejects secondary-resource
opens and allows only the declared local container/image demuxers; an actual
FFconcat regression proves playlist rejection before referenced-file decoding.

Worker soft limits are the smaller of inherited limits and 3 GiB virtual memory,
900 CPU seconds and 512 open handles. Probing is limited to 256 streams, 8 MiB of
probe input, five seconds of analysis and 16 MiB native index budget. Decoders use
two threads, at most 8192×8192 pixels, 64 audio channels and 768 kHz sample rate.
The source picture index is capped at 500,000 pictures; native sound blocks at
65,536 sample frames. Core document validation and the native workspace's 8 MiB
serialized-document budget apply before publication. Limits fail explicitly.

The UI owns one import job. Hashing, probing, decode, command validation and final
verification run off the UI thread. The final editor commit requires the captured
tab session, generation and document revision. Edits/close/obsolete jobs cannot
replace newer work. Cancellation reaches native IO and decode; a worker that
ignores it is killed after two seconds and reaped off the UI thread. Worker death
wakes an idle stream-selection window, reports an error, and keeps the document.
The next normal import can retry after an error or cancellation.

## Current scope

Software fixtures qualify explicit multi-stream/VFR FFV1, delayed H.264/AAC,
PCM integer/float sound, declared and unspecified six-channel layouts, and single
PNG/JPEG pictures with source alpha. Real H.264 portrait and H.264/AAC camera
sources have separate indexed seek and native recovery receipts in
[ingest evidence](evidence/r2-media/README.md).

Picture conversion is CPU RGBA8. Tagged supported YUV matrices/ranges are used;
unspecified HD/SD matrix interpretation uses BT.709/BT.601 respectively. Original
source tags and bytes are retained. This does not qualify float/HDR source decode,
managed display/output color, RAW SDKs, image sequences, hardware texture handoff,
proxies/waveforms/thumbnails, composition playback/export, long sound synchronization
or independent production jobs. EB-021 and R2 remain partially implemented.
The real portrait's uncached CPU seek misses the predeclared warm-seek budget;
cache and scheduling qualification must close that gate without relaxing it.

Retained picture playback now has a separate packaged Rust endpoint over private
Unix sockets and sealed binary planes; see [isolated pictures](WORKER_PICTURES.md).
It shares the indexed decoder/source/cache and GPU graph while preserving this
ingest path. Native preview, audio scheduling and delivery integration remain open.
