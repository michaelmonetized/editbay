# Editable caption tracks

Native caption tracks use shared document commands for UI, CLI, undo and recovery. Older schema-2 documents default the caption collection to empty.

The editor's Editorial exchange panel accepts the exact EDL frame-rate fraction before import because EDL carries no rate. Select retained original media and its moved path to relink only after a complete-byte fingerprint match. The same background-job ownership checks protect import, export and relink results.

Captions import and export UTF-8 SRT/WebVTT with exact rational native cue timing. Add, edit, split at the program playhead, retime, select safe margins and retain a font. Track delivery chooses preview, SRT/VTT sidecars, burn-in or both. Sidecars are clipped and rebased to an export range and published beside the verified master. Existing destinations are refused. Standard subtitle files cannot represent submillisecond boundaries: export rejects that loss instead of rounding. VTT identifiers, settings, comments and STYLE/REGION blocks survive interchange; unsupported rich caption burn-in fails visibly. Burn-in uses shaped Unicode glyphs from a fingerprinted retained font; missing glyphs fail. Preview safe guides use the same retained margins. Choose a retained font to preview text: sidecar and preview-only tracks then use the same Unicode shaping, colors and layout as burn-in on the viewer worker.

`editbay captions-import FILE COMPOSITION_ID SOURCE_SRT_OR_VTT` and `editbay captions-export FILE TRACK_ID NEW_SRT_OR_VTT` expose the shared import/export path. Shared `set_caption_track` commands retain exact edits and undo.

Cue text stays in a local draft while typing. Apply text or leave the field to commit one valid edit; unfinished multiline text remains editable with its validation message. Applied text survives native save/reopen; switching track or document discards unapplied drafts. Caption file paths require `.srt` or `.vtt`, and complete sidecar bytes publish atomically without replacing an existing file. An unsupported optional preview shows a warning and keeps the program picture; required burn-in still refuses unsupported content.

Focused software checks cover exact cue timing, Unicode, safe-area bounds, shaped glyph coverage, stale commands, undo, save/reopen/recovery and sidecar range preparation. Native UI/GPU delivery is a separate proof boundary; no desktop, sound-device or hardware qualification is claimed.
