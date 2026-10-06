# Canonical source sound

Issue #53 follows #51 / PR #52. Source PCM is decoded sequentially from its
presentation start and retained in private anonymous files. Cold late reads,
backward access and overlapping intervals use the same original float samples.
A decoder evicted from memory reopens at the beginning and advances through its
history before extending an existing store; container seeks never replace the
canonical sample history.

`PcmBudget` separates original memory/pin/interval limits from disk storage:
8 GiB and 16 storage handles by default, with accepted maxima 8 GiB / 32 handles.
The existing 8 source handles and 2 decoder handles remain. Disk bytes count actual
retained float payload; anonymous files close on clear, rebind, process retirement
or death. Consumer-held sealed PCM keeps its independent memory/handle charge
after those files disappear. `NativePcmCache::new_in` selects the storage directory;
the default honors the process temporary directory. Every qualification command
sets `TMPDIR` under the project artifacts directory.

`prepare_interval` performs at most the configured `decode_frames` in one native
step, retaining actual decoded/required source-frame progress. Native synchronous
interval reads repeat these bounded steps. The supervised PCM process returns
typed intermediate replies; the parent checks request ownership, interval, rate,
totals, progress monotonicity and resource ceilings before continuing. PCM still
travels through the existing immutable sealed mappings.

Playback observes one fixed-size source-progress record outside the device
callback and displays actual source seconds during startup preparation. The
30-second startup guard now measures absence of source progress; completed native
steps can keep a long preparation alive while cancellation remains available.
Device callbacks retain their original 16384-frame prepared queue, 4096-frame
producer blocks and 20 ms continuity bound. They gain no file IO, decoding,
allocation or locking. Preview and isolated delivery use this same native cache.

Local tests cover a 12-second multichannel AAC source, late/backward/overlapping
reads, decode-work bounds, decoder eviction, storage limits, cancellation,
source changes, output pins and cleanup. The actual codec worker reports partial
preparation, survives explicit retry after cancellation/death and matches full
independent sequential PCM. Malformed/foreign/stale progress is rejected.
`editbay-lab canonical-pcm SOURCE [WORKER_BINARY]` measures ten fresh late reads
and 320 warm random reads against independent FFmpeg PCM; its unchanged gates are
1-second cold / 250 ms warm p95, exact sample bits and bounded storage/cleanup.
Real camera/multichannel, full output and native workflow evidence is pending.

Preparation is currently demanded by requested source intervals. Cold future
source jumps during active playback can still require more preparation than the
device queue covers; preparation ahead of those cuts and sustained qualification
remain open R2 work. This dependency does not establish physical audibility,
two-hour clock continuity, other hardware or complete R2/R8 qualification.
