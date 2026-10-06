# Exact picture preparation

Issue [#41](https://github.com/michaelmonetized/editbay/issues/41) follows
[PR #40](https://github.com/michaelmonetized/editbay/pull/40). The native viewer's
**Prepare playback** control prepares the selected sequence's exact source
pictures in the background. It shows progress, cancellation and failures. Editing,
changing tabs or selecting another sequence discards that preparation. Play still
uses the same GPU graph, bounded future-picture queue and sound clock.

This prepares source access for dense cuts. It does not promise that arbitrary
effect graphs meet the playback deadline. A source change, missing file, invalid
cache, failed worker or missed picture deadline remains a visible failure.

## Ownership and limits

`StorePlan` captures one immutable document, composition and nonempty frame range.
It follows the core operation dependencies from the picture root, resolves exact
nested and reverse time, and deduplicates source pictures by source bytes/hash,
stream interpretation and indexed ordinal. Sound-only paths are excluded. Source
decode order changes; timeline order and sound samples do not.

Planning rejects more than 1,000,000 frames/evaluated nodes, 32 nesting levels,
the existing source-handle/geometry limits, 4096 unique pictures or 8 GiB of RGBA8.
The complete plan must fit before storage is created. The existing decoded,
mapped, consumer-handle and GPU live budgets stay unchanged. No proxy or alternate
renderer is introduced.

`StoreSpace` reserves the complete byte and entry allowance across unfinished
jobs, ready results and readers. A retained reader keeps the old pack charged
when another preparation replaces it. Exhausted storage fails explicitly. The
ordinary decoded and mapped picture caches retain their existing eviction and
consumer-pin accounting.

## Files and verification

The Rust background job creates an anonymous `O_TMPFILE` beneath the configured
application state directory. It writes exact native RGBA8 pictures through the
supervised codec provider, retaining original picture metadata and SHA-256 for
each payload. It verifies used sources before producing a ready result. A ready
pack has no pathname and is reopened read-only after its writer closes.

The packaged codec receives one read-only descriptor and a bounded typed index
over its existing private transport. It rejects named/writable/nonregular files,
foreign document versions, duplicate entries, invalid offsets/lengths and source
selection or metadata mismatches. Color, alpha, interpretation-required status
and rotation must match the captured source profile. Each disk read checks its
full picture checksum before sealing the normal shared-memory output plane.

Cache hits still open or validate the original source's identity. A cached copy
does not make a missing or changed original acceptable. Decoder and consumer
process ownership, ordered messages and stale-result rejection remain enforced.

Cancellation checks occur while planning, between source operations and every
64 KiB of pack IO. The existing codec supervisor handles stalled/dead native
children. The UI polls completion without joining active work. Discarded results
cannot publish into another document, sequence or job. Dropping the final reader
closes the anonymous pack and releases its reservation; cleanup has no source or
destination pathname to unlink. Packs are ephemeral and are not advertised as a
durable, reopenable disk cache.

## Qualification

`editbay-lab picture-store PROJECT COMPOSITION NEW_DIRECTORY WORKER_BINARY`
compares every planned picture with an independent sequential FFmpeg decode,
then exercises stale binding, killed/stopped workers, partial cancellation and
storage cleanup. `native-cached APP SAVED_LONG_PROJECT NEW_DIRECTORY` exercises
actual preparation/cancel/failure/edit invalidation, dense playback, numbered GPU
completion receipts, seeks, edit history, save/recover/reopen and shared delivery.

The initial ARM64 debug candidate's checked disk reads took 134.450 ms p95. That
result is retained. SHA-256 now uses optimization level 3 in the development
profile. Core planning and sound computation use optimization level 2 in that
profile after the first native candidate retained a 71.299 ms input miss and a
six-channel sound underrun. Algorithms, digest formats, verification, buffer
budgets and the release profile are unchanged. Runtime evidence identifies the
actual profile and binary; earlier failures remain recorded.

The scoped gates remain prepared-hit seek p95 <=250 ms, cancellation/retirement
<=2 s and sampled baseline app/descendant RSS <=4 GiB. Every-frame completion,
input timing misses, preparation cost and memory boundaries are reported
separately. Physical presentation, audibility, long hardware drift, 1080p/4K
hardware coverage and the rest of R2–R11 remain independent release gates.
