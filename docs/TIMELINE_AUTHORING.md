# Linked timeline authoring

This is the first EB-030/031 editing layer. It creates real reusable compositions
through ordinary version-owned document commands. There is no second persisted
edit model. Source compositions and original files remain unchanged.

## Native workflow

Import media and create a source sequence. In Timeline, choose that source and
mark an exclusive In/Out range. `View source` uses the existing native GPU viewer;
I marks the displayed frame as In and O marks one frame after it as Out.
`Create cut` starts a linked V1/A1 assembly with that range.

Choose the record cut and a record frame. Append (E) places the selected source
range at the last clip end. Insert (W) splits a clip when necessary and shifts
following linked groups. Select a row to split (S), trim its source range, move
the group, or remove it (Delete). Ripple trim/removal shifts following clips by
the exact length change. Moving does not overwrite another clip: overlap fails.
`Use playhead` copies the viewer frame only when the viewer shows that record.
`View record` displays the requested frame. Gaps render transparent picture and
silence. Existing Play/Pause, save, undo/redo, recovery and Export use this graph.

All edit planning, graph inspection and sound-budget validation run on Rust
threads. Publication checks the captured tab, document version and session
ownership. Each successful edit is one undo group; conflicting work fails
visibly. Edit shortcuts are suspended in text fields and behind dialogs.

## Exact contract

`timeline_edit(Project, TimelineAction)` returns a validated command group plus
composition/clip selection. It supports Create, Insert, Split, Trim, Move and
Remove. `timeline_clips` derives a view from existing tracks and graph nodes; it
rejects graphs with additional operations or animation instead of discarding
those edits. Every sound-bearing source has reciprocal picture/sound clip links
with identical record range, source composition and time map.

This layer requires matching source/record dimensions and rational frame rate.
Clip maps are exact unity-rate integer composition frames. Nested sources retain
their own picture timing, delayed sound, preroll and sound tails. Channel layouts
must match; implicit mixing/conversion is rejected. Source ranges are exclusive,
nonempty and within source duration. Checked time arithmetic rejects overflow.
Original node/clip identities survive unaffected edits; split creates a new
right-hand pair and keeps the left-hand pair.

A canonical cut contains one video track, its paired audio track, transparent
background, timed source/over picture nodes and timed source/mix sound nodes.
The normal project validator checks types, source bounds, reciprocal links,
identity uniqueness and graph cycles. Sound compilation plus a maximum block
preflight rejects edits exceeding current compiled-path/preparation budgets.
[Sound interval indexing](SOUND_INDEX.md) now separates compiled history from
active block work. The canonical cut still uses one mix node with at most 256
inputs; broader authoring and long-job qualification remain open.

The document requires positive composition duration. Removing the last group
leaves a one-frame empty canvas, clearly labelled Empty cut; append starts at
zero and delivery rejects it until a source range exists.

## CLI

`editbay clips FILE COMPOSITION_ID` prints ordered derived clip selections.
`editbay timeline FILE REQUEST_JSON` applies one action and conditionally saves
only if the loaded file is unchanged. The request contains `expected` in the
same DocumentVersion format as ordinary command groups, and `action`, for example:

```json
{
  "expected": {"project_id": "PROJECT_UUID", "revision": 3},
  "action": {
    "kind": "create",
    "name": "Client cut",
    "source": {
      "composition": "SOURCE_COMPOSITION_UUID",
      "range": {"start": 16, "end": 28}
    }
  }
}
```

Use the actual expected version fields from the saved document/command receipt.
Stale versions, unknown fields and requests larger than 64 KiB fail without saving.

## Qualification

Core checks cover inserted and fractional cut/sample boundaries, silent gaps,
linked identities, ripple/move, overlap/overflow/recursive rejection, save,
undo/redo and recovery. Native ownership tests exercise revision changes,
tab switches, closed tabs and hidden workspaces.

`editbay-lab native-timeline APP PROJECT NEW_DIRECTORY` drives the real native
window through create, append, split, remove, trim, undo/redo, cut-boundary GPU
viewing, streaming playback, save/recovery and a new exported MOV. It requires
at least 65 source frames and writes actual diagnostics and a screenshot.
`timeline-compare PROJECT COMPOSITION UNCUT_MASTER CUT_MASTER` independently
decodes the corresponding uncut master slices with installed FFmpeg and compares
the concatenated picture/PCM hashes. This specific independent comparison requires
integer sample boundaries at 48 kHz and a contiguous cut of one source master;
core fractional timing checks are separate mathematical evidence.

[Final native and media receipts](evidence/r3-timeline/README.md) qualify camera
and six-channel cuts, exact independent picture/PCM slices, reversed source
order and saved/recovered file equality. Full EB-030, R3, client acceptance and
release readiness remain open. Precision roll/slip/slide, overwrite,
mixed-rate conform, routing, multiple editable tracks, dedicated simultaneous
source/record monitors, range export and long-job acceptance continue afterward.

`timeline-reorder CLI PROJECT COMPOSITION UNCUT_MASTER NEW_DIRECTORY` moves a
saved two-clip cut into reverse source order through actual CLI requests. Every
request is replayed with its now-stale version and must leave saved bytes unchanged.
It then exports and compares all picture/PCM bytes to independent source slices.
