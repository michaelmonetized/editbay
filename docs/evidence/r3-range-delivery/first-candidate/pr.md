Native export options and `editbay export-range` now select a half-open composition frame range through the shared supervised picture/sound graph. At rate N/D and sound rate S, [A,B) selects original-grid samples [ceil(A*S*D/N), ceil(B*S*D/N)); MOV timestamps restart at zero without changing that selected sample count. Full-sequence export remains the default.

The parent and worker validate the interval and returned profile. Existing ownership, cancellation/retry, private decode verification and non-overwriting publication remain in the delivery path. Native options reject invalid input before the destination chooser.

Stacked on #48 (`stack/r2-sound-backend`). Closes #49 only when the recorded acceptance checks complete.

Validation in progress on frozen runtime `b360f89`: actual camera/six-channel CLI slices at 48 and 44.1 kHz with independent FFmpeg comparisons; native chooser/compact-window checks; worker faults and protocol rejection; locked default/all-feature workspace tests, fmt and all-target/all-feature Clippy. Targeted codec tests already cover 12 actual round trips and the fractional one-sample boundary. Full R2–R11, sustained playback and release/adoption gates remain open.

The first qualification passes every camera range and all 14 fault/protocol groups, but detects a real six-channel tail mismatch caused by starting compressed decoding mid-source. This draft now adds cancellable bounded sound pre-roll with validated progress so range PCM follows the same decode history as a whole export. Fresh qualification is required; the failed first candidate remains recorded.
