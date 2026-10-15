# Editorial interchange

`editbay exchange-import SOURCE NEW_PROJECT FORMAT [OPTIONS_JSON]` opens CMX EDL, FCP7 XML, FCPXML, OTIO, AAF and OMF as native editable compositions. `editbay exchange-export FILE COMPOSITION_ID NEW_FILE FORMAT` writes the selected cut and a required `.loss.json` report. Formats are `edl`, `xml`, `fcpxml`, `otio`, `aaf`, `omf`.

Imports probe and fingerprint the actual linked files. Missing media requires an explicit `relinks` path map in the options JSON; no invented source profile is accepted. `edl_frame_rate` supplies the EDL's unstated rate. Record boundaries must fit the declared frame grid. Source boundaries stay rational, including reverse/freeze/constant speed. AAF picture retimes use standard VideoSpeedControl/VideoRepeat operations and exact referenced source clocks; audio retimes in AAF/OMF remain explicitly loss-reported. Unsupported variable speed export and effects are reported. Native linear picture/sound dissolves retain their cut and handles. Leading, internal and trailing gaps are retained. Source handles outside actual media fail visibly.

`editbay relink FILE ASSET_ID MOVED_ORIGINAL` verifies all bytes and preserves source identity and timing. A changed replacement is rejected. Save/reopen and undo use the shared document commands, with additive default-empty transition data for older schema-2 projects.

The portable format implementation is pinned to FilmCraft 8309f21a6c3f34ddf190af7fd9bfd913053a1fd5 under MIT; see `vendor/filmcraft/README.md`. No FilmCraft codecs or UI are used. Embedded AAF/OMF essence extraction is not currently exposed; the report records that loss and imports require linked original files. OMF's audio-only format limits and other unsupported format features are retained in reports.

Focused checks cover format bytes, native reopening and moved-original fingerprinting. These checks do not demonstrate another NLE opening every format, native UI operation, physical playback or hardware compatibility.
