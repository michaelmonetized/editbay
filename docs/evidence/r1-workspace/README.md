# R1 native workspace evidence

Local ARM64 validation on Apple M1 Pro, Linux/Omarchy, Rust/Cargo 1.98.0,
Hyprland Wayland and the installed Synchro portal, 2026-10-03. Local R1 gates pass.
Production media authoring and enterprise/GTM release remain later gates.

## Provenance

The final native recovery candidate is an unoptimized local Cargo build, SHA-256
`79d6903ff0dacd7d687f5666426944beafa7f6d307922e8c08060c2fabe03c8d`.
The native Rust qualification driver is
`81a40f9853d90a06059d72c90fd78d6cfe698cbb5c276d071bc6cf0fc5da94bf`.
The lockfile is
`25f9789a4caf019278c7a0e1c7bfdaf2c7e6e20662979b95ec6164c33f79f11c`.
`source-hashes.txt` records the application, persistence changes, driver and
narrow vendored SDK adapters. The containing commit is the source revision;
large binaries, diagnostic traces and client assets remain outside Git.

The four welcome captures and storage-error captures use the final candidate.
Portrait-profile and brand captures use the preceding candidate
`16404d6da0a5492eadbf2cd24f71ec7bc42fe662c247e90dbd9dfdf8edd7e138`.
The final candidate additionally surrenders name-field focus on modal closure
and records narrow shortcut/modifier diagnostics when explicitly enabled.
IME captures use candidate
`33621988b7778e7fb53baa5b16c1ee179f5734f598874314dcf48a4066000270`;
the clipboard/IME SDK implementation is unchanged between these candidates.

## Local software checks

`tests-workspace.log`: 67 tests and 5 SDK documentation examples passed.
`tests-all-features.log`: 68 tests and 5 documentation examples passed, with
native libtorch selected. Both have zero failed or ignored tests.
`clippy-all-features.log` and `cargo fmt --all -- --check` passed.
Cargo used `/var/tmp/editbay-build` with incremental compilation disabled.

## Native qualification

`qualification.json` records 100/100 actual process-kill, welcome/recovery-preview,
separate-copy recovery and native reopen trials while cataloging 4,000 real files.
Fifty trials start with untitled work and fifty with saved originals. Each verifies
active and inactive acknowledged checkpoints, revision 3, independent recovery
identity and unchanged original/checkpoint hashes. The driver found no dropped
diagnostic records. The qualification captures 250 input samples: p50 11.768 ms,
p95 25.086 ms, maximum 65.256 ms, passing the p95 <= 50 ms gate. Two hundred durable
UI commits have p50 0.241 ms, p95 1.059 ms and maximum 4.585 ms. The 14,935 UI CPU
frames have p50 0.967 ms, p95 5.455 ms and maximum 264.073 ms, including startup.
Peak process RSS/HWM across the trials was 103,248 KiB.

`timing.json` records 23 continuous edits with every interval below 1 s. The first
durable checkpoint published at 9,807.159 ms, inside the 10 s maximum. The latest
revision published 1,051.024 ms after the last edit: the 1 s debounce plus at most
250 ms preparation/publication allowance. Input p95 was 16.844 ms.

`storage-errors.json` records actual EACCES (13) and ENOSPC (28). ENOSPC uses an
owned 4 KiB tmpfs inside a private user/mount namespace. Both errors remain visible,
leave work dirty, acknowledge no checkpoint, and publish a valid SHA-256-checked
checkpoint after repair. The captures show the real application errors.

Commands used the frozen binaries above and fresh, nonexistent output directories:

```sh
editbay-lab native-workspace editbay-studio /var/tmp/editbay-native-qa/qualification-100-v5 100
editbay-lab native-workspace-timing editbay-studio /var/tmp/editbay-native-qa/recovery-timing-v3
editbay-lab native-workspace-errors editbay-studio /var/tmp/editbay-native-qa/storage-errors-v2
```

The final welcome was inspected at 800x600 and 1440x900 in the current Hurleyus
dark palette and official Flexoki Light palette. Palette switching changed only
an application QA override file; desktop settings were preserved. The current
Omarchy dark color-file hash was
`a7eddcf3342ee0bee5df5ed7aad71c0e8c07ba5549bba7e040064fc5abc09eea`;
`flexoki-light-colors.toml` records the official light file, hash
`bb3b5c43fc247f7e2f874c2b5b8e1c32f102245e55addb63bb0a2e7df9351ec7`.

## Native input and assets

The actual Fcitx Unicode addon emitted seven preedit events in four UI frames,
then one commit event for U+20AC. The first Return committed the character and
kept the New Project modal open. Native Ctrl+V and Shift+Insert both delivered
the existing clipboard text with normalized line endings and the 256-character
field limit. Clipboard contents were preserved and omitted from public evidence.
These routes do not qualify every keyboard, language engine or physical device.

The native picture-profile workflow selected 1080x1920 at 24000/1001 fps, saved
through the installed portal, and reopened those exact settings. Source project
SHA-256: `cd7c237e65e2dcc4892b1ac629405127e4d58f2da72e39f60409a1c28da103db`.
Both inspected window sizes preserve the portrait aspect; the compact document
view scrolls when its contents exceed the available height.

Three successive real portal requests in one application process imported a
client SVG, exported an independent copy and imported an installed font. The SVG
source, stored asset and exported copy agree:
`436c5be23bb3b237124f798eb74b51e83a2204c381c20c2a43da542d40e662e8`.
The font source/stored asset agree:
`0ec29a68b539ece7078fc714cebff0c0accb2f4948f8f7963d9f5e86633b12d9`.
Import preserved unsaved client/project/notes and palette fields; explicit Save
published them. A native window-close request with newer unsaved bank details
was cancelled with a visible Save/Discard notice; the saved manifest retained
the previous client and both asset receipts. Discard restored its saved state.
The preexisting legacy `.omabrand` directory was retained; no shared Omadesign
manifest was replaced. Asset application/rendering belongs to later milestones.

## Failures found and corrected

Earlier native attempts exposed first-pass modal input loss, pointer coordinates
below the tab, name focus surrendered by the opening click, portrait canvas
distortion, a stalled second portal request after dropping its Tokio runtime,
unsaved bank drafts replaced by worker results, text IDs changed by conditional
status rows, and closed name forms retaining text focus before immediate Undo.
Application defects now have focused regressions; the native driver waits for
observed owner/modal state and pointer movement delivery. A three-trial smoke
with 4,000 real catalog documents passed on the preceding candidate, with
input acceptance p95 35.743 ms. Failed attempts are retained locally and never
counted as completed qualification trials. Later diagnostics showed that missing
injected shortcuts had no corresponding application key event. Workspace chords
and Enter now use the persistent Linux input device. A later run completed 81
trials before a missing chooser-navigation chord saved a valid recovery fixture
into the chooser's default folder. That owned fixture was retained with the
failed-run evidence; the original stayed intact. Chooser chords now use the same
persistent device, while `wtype` supplies text only. The initial smoke passed with input
acceptance p95 19.385 ms. This changes the test injection route, not application
shortcut semantics. [wtype's source](https://github.com/atx/wtype/blob/master/main.c)
documents its per-process virtual keyboard/keymap lifecycle.

## Qualification limits

Native input uses software-injected Wayland text/portal keys and persistent Linux
keyboard/pointer events.
Recovery exercises actual process SIGKILL, local durable files, the native
welcome/recovery preview and Synchro separate-copy dialog, followed by file reopen.
This is not power-loss, physical input latency, completed client-edit, playback,
cross-hardware or live-service qualification. R2–R11 and independent GTM job
acceptance remain open.
