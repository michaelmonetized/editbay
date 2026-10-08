# EditBay shipping candidate

This candidate completes the local editing and delivery routes described below.
It does not close every milestone in the production-suite roadmap. Release
qualification is recorded separately from implemented behavior.

## Editing and finishing

Paired picture/sound tracks retain reciprocal clip identities. Source and record
ranges use exclusive integer boundaries. Insert, overwrite, split, trim, move,
ripple removal, slip, roll and slide use typed commands and one undo group per edit.
Overlapping placement on a track fails visibly; layers use additional track pairs.
Independent sound placement supports a separate music or dialogue mix.

Conformance wraps the original sequence with a rational time map. Sound keeps its
original elapsed time. A fractional final frame is padded rather than stretching
the source. Coarse container timestamps retain continuous decoded PCM when their
rounding matches consecutive samples; discontinuities outside that resolution fail.

Clip controls include translation, scale, rotation, opacity, linear gain, exposure,
contrast and saturation. Editable titles retain a fingerprinted font asset. Title
text currently supports ASCII, 1–1024 characters, with bounded rasterization.
Picture/sound fades remain ordinary animated graph nodes. Unsupported custom
timeline graph structures are rejected before an edit changes the document.

## Checked delivery

| Profile | Picture | Sound | Limits |
| --- | --- | --- | --- |
| H.264 MP4 | libx264, 8-bit YUV 4:2:0 | AAC | Even dimensions; mono/stereo, 44.1 or 48 kHz; alpha over black |
| ProRes MOV | ProRes 4444 with alpha | Float PCM | Original channel layout; declared SDR profile |
| Lossless MOV | PNG RGBA8 | Float PCM | Exact decoded picture and PCM hashes |

Compressed output is decoded and compared against the same graph for every frame
and sample. H.264 picture RMS error must be <=0.035, ProRes <=0.015 and AAC <=0.02.
These numerical bounds are software QC, separate from a reviewer approving the edit.
Outputs declare BT.709 primaries and sRGB transfer; a station requiring another
transfer or delivery specification needs that explicit profile.

The native queue retains at most eight jobs and starts one codec worker at a time.
Each job captures its document version. Edit, undo, save, replacement and close
invalidate affected jobs. Queued cancellation does not stop the active job.
Existing destinations are never replaced. Publication follows source verification
and complete output QC; cancelled or failed work cannot publish a partial file.

Native tests exercise queued cancellation and automatic start, process death,
edit invalidation, retry, independent decoding and worker retirement.

## Portable projects

File → Archive with media creates a new folder, streams and verifies every retained
asset into `.editbay-media`, then publishes `Project.editbay` last. Stored asset
paths are relative to that folder. Moving the folder preserves reopening, editing,
recovery and export. Original assets are read only. Existing archive destinations
and changed source fingerprints fail visibly.

```sh
editbay archive Cut.editbay NewArchive
editbay info NewArchive/Project.editbay
```

## Native installation

`editbay-install` packages the built ARM64 executables, their resolved native
library closure, PipeWire/SPA modules, configuration, a retained title font,
locked Rust package notices and installed native dependency licenses. Packaging
runs from the source checkout with native-target dependencies already fetched.
The manifest declares architecture, version,
file sizes, executable permissions and SHA-256 hashes. The launcher uses the
version's bundled libraries and font. The host supplies glibc, its loader, GPU
drivers, the audio server and desktop/file chooser services.

```sh
editbay-install pack target/release NewBundle 0.1.0-rc1
sha256sum NewBundle/manifest.json
editbay-install install NewBundle InstallPrefix MANIFEST_SHA256
InstallPrefix/current/bin/editbay
editbay-install verify InstallPrefix
editbay-install rollback InstallPrefix
```

Install verifies the pinned manifest and every staged file before atomically
switching `current`. The previous version remains available. Install and rollback
retain user projects and recovery data. Corrupt updates fail before switching.
The ARM64 bundle retains 392 locked Rust package notices. The checked installation
passes a real update, rollback in both directions, unchanged project/checkpoint
hashes and native playback of every picture and sample of the 30-second stream
job. A corrupt update is rejected before switching the installed version.
Installation is qualified on the current ARM64 Omarchy host; another architecture
requires its own build and native qualification.

## Playback qualification

Native PipeWire float playback fills device buffers on the real-time process
callback from a fixed sound ring. Preparation and its PCM child request bounded
real-time priority after source preparation. Promotion is measured in actual thread
schedules. No callback allocation, file I/O or source decoding is added.

The stream requests and locks a 2048-frame quantum only while it is active.
Backend presentation time includes native graph delay and queued/buffered samples.
The unchanged continuity gate stops visibly above 20 ms instead of skipping source
sound. Device changes, underruns and child failure retire owned work.

The two-hour production runtime completes at exactly 345,600,000 samples, with
zero driver underruns/dequeue misses and reaped children. Maximum backend/host
drift is 0.194580/0.194913 ms; peak sampled combined RSS is 145,344 KiB. The route
uses measured adaptive rate correction, resident realtime audio services and
interrupt-driven protected speaker output.

The frozen recorder retried four disk writes. Its zero-retry recording check
fails. Independent audit parses all 70,869 expected observations, recomputes
clock/memory summaries, verifies the byte count and matches the stored trace
hash on a second read. The current collector fails on any recording error.
Earlier failed runs remain local evidence; no zero-retry recording pass is claimed.

The installed RC9 stream completes all 900 actual GPU draws before observed sound
end. Selection-to-draw p95 is 9.053 ms. After verified rollback and restoration,
playback completes 896/900 draws with four counted skips; p95 is 13.603 ms. Slow
UI frames trigger the existing policy of selecting the current sound-clock frame
and discarding expired pictures. Both runs reach exact audio end, preserve sources
and reap workers. This records native scheduling and its degradation, separately
from physical display timing and the broader hardware matrix.

## Real-job acceptance

Local commercial, social and camera/stream projects have been authored through
the shared Rust commands and exported through native delivery. The stream replaces
the interview category at the owner's request. Commercial and portrait revisions,
a fractional-rate ProRes master, an Omadesign process social and the 30-second
camera/stream segment have native QC and independent null decoding.

The stream source is a packet-copy excerpt of an actual recording. The original
is retained and fingerprinted. Whole source indexing is currently bounded to
500,000 pictures; the long 60 fps recording exceeds that bound, so preparation of
the excerpt is disclosed rather than claiming whole-recording import.

The installed RC9 candidate passed 100/100 native kill/recover/reopen trials with
4,000 catalog documents, including 50 saved and 50 untitled originals. Input p95
is 16.992 ms across 250 injections, with a 23.458 ms maximum. CPU frame work p95
is 0.556 ms. Checkpoint acknowledgement on the UI takes at most 0.002 ms; durable
worker commits have p95 4.030 ms and maximum 1,021.294 ms. Actual native startup
has maximum 5.778 seconds and storage completion waits maximum 8.062 seconds.
The harness reports separate 30-second startup and 120-second storage deadlines;
input acceptance retains its 50 ms p95 gate. These are software-injected native
events. Original/checkpoint bytes and all 500 archived diagnostic capture hashes
pass independent verification. The native file chooser confirms actual focus,
cursor changes and a validated filename before accepting Save; earlier failed
attempts remain separate evidence. See [qualification](NATIVE_QUALIFICATION.md).

Each of the three acceptance jobs passes checkpoint recovery, a moved media archive
and matching lossless range picture/PCM hashes. Private media and original paths
remain local. The three requested review videos are uploaded unlisted and each
plays to its end on YouTube without a reported playback error. Review links remain
in the owner's thread and local receipts. The owner accepted all three release
jobs after reviewing these links.
The [release evidence](evidence/shipping-candidate/README.md) records the five
scoped gap implementations and measured qualification, including all limitations.
Station/client approval,
physical speaker/display timing, other GPU families and the broader suite's
migration, recording, loudness, team/cloud and artist workflows are not implied
by these local receipts.
