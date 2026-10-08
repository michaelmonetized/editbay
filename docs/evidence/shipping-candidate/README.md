# ARM64 release qualification

EditBay 0.1.0-rc9 is installed on the qualified ARM64 Omarchy host. The five
local release gaps in [#61](https://github.com/michaelmonetized/editbay/issues/61)
have working implementations and recorded qualification. The owner accepted all
three unlisted commercial, social and stream jobs. Private media, paths and
review links remain in local receipts.

| Scope | Actual result |
| --- | --- |
| Sustained audio | 7,200 seconds; exact 345,600,000-sample end; zero underrun ticks/dequeue misses; backend/host drift 0.194580/0.194913 ms; peak combined RSS 145,344 KiB; reaped children |
| Native recovery | 100/100 kills, recoveries and reopens; 4,000 disk-backed catalog documents; 50 saved and 50 untitled originals; exact originals/checkpoints; input p95 16.992 ms, max 23.458 ms |
| Native pictures | Initial stream: 900/900 actual draws, selection-to-draw p95 9.053 ms. After rollback: 896/900, four skips, p95 13.603 ms. Both exact audio end, preserved sources and reaped workers |
| Editing/delivery | Shared typed multitrack editing, undo/save/recovery, editable finishing; H.264/AAC, ProRes/PCM and exact lossless RGBA/PCM; native queue/cancel/death/retry and independent decoding |
| Installation/jobs | 1,355 checked bundle files and 392 locked Rust notices; verified actual user update and RC9 → RC8 → RC9 rollback; unchanged settings/documents/checkpoints; corrupt update rejection; three accepted real jobs with exact recovery and moved-archive exports |

The two-hour frozen recorder recovered four write errors. Its strict zero-retry
recording check fails. A separate complete audit parses all 70,869 expected
records, verifies declared 151,780,374 bytes, recomputes drift/memory summaries,
checks original source/project hashes and matches a second read of the stored
trace. The newer collector aborts on every recording error. This is an audited
runtime pass with disclosed recorder retries, not a zero-retry recording pass.
Trace SHA-256: `ae16ec33510ea3623c0e3e7f329d8bdd89b86177ed9efdd805f3f2de3c7d6e49`.

The second picture run skips frames 24, 41, 80 and 377. UI frame intervals include
40–70 ms stalls; the existing catch-up policy selects the current audio-clock
frame and counts expired pictures. This degradation remains recorded alongside
the initial complete run. Picture preparation is bounded to eight queued images,
with first draw before sound starts. No physical display or full R2 claim follows.

Native recovery captures diagnostics on a bounded 512 MiB non-swapping tmpfs,
then copies and syncs them to disk after each owned process is stopped. All actual
projects, originals, checkpoints and recovered copies remain on Btrfs. Independent
checks match all 500 archived capture files and parse 109,208 observations with
no dropped records. CPU frame work p95 is 0.556 ms; UI checkpoint acceptance max
is 0.002 ms. Durable worker commit p95/max is 4.030/1,021.294 ms. Startup max is
5.778 seconds; storage completion wait max is 8.062 seconds. Separate harness
deadlines are documented in [native qualification](../../NATIVE_QUALIFICATION.md).

The frozen audio engine and final installed production sources match, except for
the first worker request's five-second startup deadline; subsequent requests retain
500 ms. The two-hour run passed the stronger original startup deadline. Final
workspace locked tests, production Clippy and formatting pass; the latest Rust
qualification helper also passes release Clippy and release compilation. Earlier
native, disk and hardware failures remain retained locally.

Installed Studio SHA-256:
`8dd35a191fbe8b939963d90cfa7f13b6b6b155faddc54cdca980849eab364de5`.
Manifest SHA-256:
`04dab15a4bf8e5ea4869314dede110d68c42580139d7275e0a77c2839304e0da`.
Machine-readable results are in [gates.json](gates.json).

Another architecture/GPU requires its own native qualification. Physical
audibility/display timing, station approval, independent artist workflows and the
broader production-suite roadmap remain separate from this local release scope.
