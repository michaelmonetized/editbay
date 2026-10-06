# Shared master qualification

Issue [#26](https://github.com/michaelmonetized/editbay/issues/26) follows native
playback [PR #27](https://github.com/michaelmonetized/editbay/pull/27). Runtime source
is `401c7d7`; `source-commit.txt`, `compiled-inputs.sha256`, `artifact-hashes.txt`,
`machine.txt` and `build.log` identify the actual local build. App, CLI and lab
executables were frozen under `artifacts/delivery/qualified-bin/`. All files,
fixtures and temporary storage stayed under the project. No own builds ran during
the final trials: **2026-10-06 02:33:38–02:35:24 UTC**.

The first profile is full-sequence MOV, PNG RGBA8/sRGB/Rec.709/full range with
straight alpha and original-channel float PCM. Read the [contract](../../SHARED_DELIVERY.md)
before selecting a workload. This is one active job with bounded history/retry,
not a multi-job delivery queue or a general format compatibility qualification.

## Actual output

| Source | Master | Sound | File bytes |
| --- | --- | --- | ---: |
| Real camera `IMG_0001.MP4` | 1280×720, 314 pictures at 24/1 fps | 628,000 mono FC samples at 48 kHz | 305,961,925 |
| Synthetic delayed six-channel AAC | 320×180, 79 pictures at 24000/1001 fps | 158,158 frames of FL/FR/FC/LFE/BL/BR at 48 kHz | 4,722,765 |

Every output picture byte and float sample agrees with the shared graph's codec
inputs. The production worker decodes and verifies its private output; the lab
then decodes the published master independently with the installed FFmpeg command.
Both paths agree exactly. Independent inspection retains exact time bases,
durations, dimensions, original channel layout, RGBA pixels and explicit color
metadata. All **57,600 pixels** of the six-channel sequence's last picture are
transparent. Neither fixture clips picture values. Sound is not monitor-downmixed.

Native saved and independently recovered projects produce identical picture,
PCM **and complete file hashes**. Their PCM hashes also match the earlier qualified
saved graph, before delivery existed. `gates.json` checks those equalities. Original
source hashes still match; see `source-unchanged.log`.

## Native workflow and supervision

The Rust driver uses the actual native window, installed save chooser and injected
keyboard/pointer input. It cancels a stopped worker, kills a worker, retries, edits
the owning document, observes cancellation, then retries the current revision.
Revision 4 exports successfully and saves/reopens with unchanged media and graph.
Viewer input continues during export; the final screenshots show the verified
master and rendered preview together.

| Native case | Four view inputs, p95 | One edit input | Cancel | Kill | Edit cancellation |
| --- | ---: | ---: | ---: | ---: | ---: |
| Camera | 19.548 ms | 9.397 ms | 125.291 ms | 9.784 ms | 109.025 ms |
| Six-channel | 19.234 ms | 8.196 ms | 126.322 ms | 7.693 ms | 110.202 ms |

Six of eight view requests were accepted while export was active. These are small
native workflow samples, not sustained-playback percentiles or physical input
latency. The pre-existing **50 ms input / 2000 ms cancellation** bounds are unchanged.
Compressed JSONL files retain the observations and focus receipts.
High-frequency delivery observations are retained in matching `.json.gz` files;
their readable `.json` summaries point to those complete traces.

Separate actual process trials pass active cancellation **6.317 ms**, worker death
**1.600 ms**, SIGSTOP fallback **172.906 ms** including its deliberate 50 ms stall,
and cancellation during final verification **8.422 ms**. Every child is reaped;
cancelled work publishes nothing. A competing destination created after rendering
remains byte-identical. Changing a private source copy during output verification
fails without publication. Nine protocol trials reject malformed/unknown requests,
missing/named/extra descriptors, stale serials, foreign job/version and duplicate
binding. The deliberately supplied named file remains empty and unchanged.

CLI SIGINT, SIGTERM and abrupt SIGKILL retire in **15.458 / 17.367 / 3.037 ms**.
The SIGKILL trial uses a lab subreaper to collect the child killed by the real
parent-death signal. No unpublished file survives; the six files in that trial
directory are its three stdout and three stderr logs. Linux anonymous storage,
not startup scavenging, supplies crash cleanup.

The recovered camera trial takes **47.224 s**, including graph render, production
verification and additional independent FFmpeg decoding. Its observed worker HWM
is **384,768 KiB**; six-channel HWM is at most **118,272 KiB**. These are scoped
process measurements, not combined application/GPU or long-project memory gates.

## Checks and limits

**170 default tests + seven doc tests; 171 all-feature tests + seven doc tests.**
No failures or ignored tests. Locked workspace tests, formatting and
all-target/all-feature Clippy pass. `checks.json` and complete logs retain commands,
environment and results. New tests cover codec pixels/alpha/PCM/clocks, malformed
input, anonymous storage, destination collisions, cancellation/publication races,
ownership/progress/receipt rejection and immediate document mutation guards.

`earlier/` preserves failed format candidates: FFV1 MOV lost the full-range
declaration; MP4 float mono lost its declared FC layout. PNG MOV passes both, with
color assigned to the encoded frame as well as its codec context. Earlier debug
workflow receipts remain private under `artifacts/delivery/`; the committed final
results use the frozen optimized binaries above.

This closes the first shared master dependency. Range export, multiple queued
jobs, additional delivery profiles, HDR/high precision, hardware encoding,
long 1080p/4K performance, two-hour hardware drift, other GPUs, installation,
client acceptance and independent users remain separate R2–R11 release gates.
