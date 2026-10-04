# Native dependency patches

`eframe` is upstream 0.36.1 with the focused Wayland redraw polling guard from
Omadesign 0.6.3, source commit `c4813ef9a9628274778047e880a3aff6ea98d199`.
Only `src/native/run.rs` differs from the published crate's Rust implementation.
The guard yields after 100 ms when every pending Wayland redraw is stalled,
retains explicit waits/timers and lets another progressing window keep polling.
Its four focused tests are retained. Upstream MIT/Apache notices and the
Omadesign MIT notice are included; EditBay does not depend on that checkout.

`egui-winit` is upstream 0.36.1 with the narrow clipboard/modifier fixes from
the same Omadesign source. Native Super remains distinct from Ctrl. Copy/cut/paste
retain the originating key and modifiers, including empty/image-only clipboard
selections. Linux clipboard-history Shift+Insert delivers its normal text paste.
The five focused chord tests are retained. Shared browser-launch code is absent:
R1 help is local. The native IME path remains upstream; canvas pinch adapters are
evaluated with their authoring controls. Native input receipts live under
`docs/evidence/r1-workspace/`; physical devices and other language engines remain
separate qualification.

Both crates are workspace members so their nine focused tests run with the
normal local suite. Their package resolver follows the workspace. The Rust 1.98
Clippy compatibility changes remove a retired lint and redundant wildcard fields;
egui-winit retains the upstream logical-size/position lint configuration required
by its explicit zoom conversion. These changes do not alter runtime behavior.
`EDITBAY_PROVENANCE.json` records the final source/manifest hashes. The retained
eframe Omadesign receipt is historical and names its original adoption scope.
