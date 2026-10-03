# R0 references and automation

Local ARM64 validation, 2026-10-03. Evidence covers the foundation and feasibility;
no commercial edit, migration importer or GTM delivery is claimed.

## Sources and ownership

`jobs.json` records two concrete source collections, required outcomes, current
receipts and missing qualification. Access is for local development/validation
under Michael's instruction. Client footage remains outside Git; no client contact
or independent approval is inferred. `macks-sources.json` contains metadata/hashes.

Actual inventory command:

```sh
/var/tmp/editbay-build/debug/editbay-lab inventory \
  '/home/michael/Downloads/iCloud Photos' \
  docs/evidence/r0-reference/macks-sources.json
```

The report records 106 sources: 41 videos and 65 stills, 107 visited entries,
one excluded entry, zero scan/decode failures and complete coverage of the declared
policy. All video records include an actual first RGBA picture hash. Stills have
byte/hash evidence only. Hidden entries and symlinks are excluded; depth/entry
limits and failures remain visible. This does not qualify full-source playback,
motion/roto quality, output delivery or rights to publish the source footage.

Inventory report SHA-256:
`5633c5905502038617578d78be56c981ba6d7bf2a77200f90a43bd461a06c01f`.
The native command checks each source's regular-file status, length, timestamp and
full hash before/after probing. Report publication synchronizes a temporary file,
refuses an existing destination and synchronizes its parent directory. Its binary
test covers hidden/symlink exclusions, decode failure reporting, an actual FFV1
input, unchanged source bytes and no-overwrite publication.

## Fixed project and migration inputs

`fixtures.json` pins every fixture hash, ownership, expected results and limits.
The native project exercises three sequence profiles and fractional frame timing.
The core reference test replays save/load/checkpoint/recovery, checks all sequence
identities/settings, refuses original/existing destinations, and compares original
and checkpoint bytes after recovery. Recovery has a new identity, retains the snapshot revision and
retains the origin identity; it does not flatten or remove sequence settings.

The authored FCPXML input defines exact fractional boundaries, a connected clip,
a marker/timecode origin, missing media and an unavailable vendor effect. It is
an acceptance input for EB-034. Current JSON loading rejects it; there is no enabled
interchange importer or output claimed here. Existing legacy editing projects were
not found in the inspected local Mack's/source directories. Interview and remaining
professional-role job inventories still need to be assembled during their milestones.

Every future conversion must produce a separate new destination and a report with:
source path/hash/format, converter commit/version, destination path/hash/schema,
preserved/editable, converted/baked, missing and unsupported features, exact timing/
stream/color changes, unresolved dependencies and errors. An unsupported feature
must name its source identity and affected range. Refusing a conversion is an
explicit failed outcome; an empty success report is unacceptable. EB-034/054/083
must replay these reports and output comparisons before advertising compatibility.

## Actual automation checks

See [native contract](../../AUTOMATION.md). `editbay-automation`'s tests launch the
real `editbay-mcp` process and use the official SDK client under modern and legacy
protocol lifecycles. They verify advertised tools, absent arbitrary path arguments,
invalid-group rollback, one revision per group, stale rejection, monotonic undo/
redo, durable file contents, immutable checkpoint, separate-copy recovery, restart,
outside edits/deletion and competing server processes. A failed save leaves the
session document/history unchanged. Sources are not uploaded by this service.

The core command tests separately cover owner mismatch, no-op groups, redo branch
replacement and revision-counter overflow. Preview/export/job endpoints remain
absent until their actual document-owned workflows are implemented.
