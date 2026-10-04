# Shared temporal evaluation

`editbay-core::EvaluationSnapshot` owns an immutable `Arc<Project>`. Create it on
a worker when the document changes, then retain it while evaluating that version.
Construction validates the complete document and prepares reachable input order,
clip/stream/asset lookup, source interpretation hashes, static operation hashes
and transitive composition fingerprints. It performs no media IO.

`prepare(composition, position, before)` returns typed `PreparedFrame` and
`PreparedNode` values. Position is a reduced exact fraction of composition frames.
Forward ranges own [start,end); reverse ranges own (start,end]. Each clip maps the
exact fractional local time into source ticks. The slope and boundary side select
forward or preceding picture/sample ownership. Freeze holds its selected source
position. Reverse evaluation at step-animation or time-map keys selects the
preceding segment. Continuous animation retains its exact key interval before
converting the interpolation offset to a floating value.

Nested requests keep their fractional position and selected source side. A
consumer prepares the child using those fields. For example, frame 1 of a 60 fps
parent can map to child frame 5/12 of a 25 fps scene. That child can select source
picture 1 at 60 fps; flooring the child to frame 0 would choose the wrong picture,
sound sample and animated parameter. Reverse nesting at the child's exclusive end
selects its last active picture/sample and preceding step value.

`Project::frame_plan` delegates to this same evaluator. The integer inspection
fields and established semantic fingerprint remain unchanged. One-shot inspection
constructs a context; playback/export should retain it. Static operation `Arc`s
are reused, so repeated evaluation does not copy polygon arrays. Animated
operations resolve only their supported parameters. VFR lookup uses binary search
in the retained timestamp index; preparation does not validate or hash that entire
index again.

## Content and publication

Node keys include their resolved operation, typed input dependencies and source
interpretation. Image-source content can reuse a held picture ordinal across
different timeline positions. Source byte hash/size, VFR/color/alpha interpretation
and actual mask asset hash/size enter dependencies. Image and mask keys also
include composition dimensions, working gamut and float precision. Display/output
settings affect the frame key; direct working-image node keys stay reusable when
only these settings change. Audio and scalar nodes retain independent point dependencies.

The frame carries `DocumentVersion` separately from reusable content identity.
Metadata edits may reuse identical content, but their previous version has no
permission to publish into the edited document. Every consuming job must still
check captured document, revision, session, generation, source ownership and
cancellation before publication. Cache hits do not bypass source preflight.

Audio source keys identify one sample position, exact source fraction and direction.
They are not sound-block cache keys. The future audio interval engine must include
its rate/channel layout, interval, complete time-map/automation dependencies and
resampling policy. Nested source keys conservatively include the entire child
semantic closure, including display/output settings and unused child branches;
these can over-invalidate the parent's nested key. Finer invalidation can follow
measured renderer needs while the unchanged child working-node keys remain reusable.

## Qualification

The CPU planning budget is set at **p95 <= 1 ms** for the declared ten-node child
plus reverse-nested parent, after 100 warmups, over 10,000 mixed fractional/direct
and nested evaluations. Initialization, source indexing and one-shot inspection
are measured separately. This is a new kernel budget; it does not replace any R2
release gate.

```sh
mkdir -p artifacts/tmp
export TMPDIR="$PWD/artifacts/tmp"
cargo run --locked -p editbay-lab -- evaluation /path/to/camera.mp4 10000 500000
```

The lab owns a read-only source descriptor and verifies its full hash before and
after qualification. It imports the first decodable picture and sound streams,
uses their actual indexed boundaries in a synthetic ten-node edit, and checks 20
integer inspections against retained evaluation. Optional stress creates explicitly
synthetic alternating 33/34 tick VFR timestamps; it does not claim those pictures
exist in the source. Receipt generation and fixture helpers remain Rust.

The [evidence](evidence/r2-temporal/README.md) separates actual source indices from
synthetic stress. Regressions preserve the published frame-24 receipt and cover
mixed rates, reverse end/step boundaries, freeze, exact sample sides, unsigned
time-map endpoints, large rational factors, shared static operations, immutable
worker ownership and required cache invalidation.

This implements temporal planning. Production pixels, audio blocks, bounded output
caches, native presentation, preview/export parity, hardware handoff, display/HDR
color, measured warm/cold seeks and two-hour drift remain open. The unchanged R2
budgets are in [release criteria](RELEASE_CRITERIA.md).
