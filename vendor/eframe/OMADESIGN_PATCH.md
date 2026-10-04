# Omadesign Wayland redraw scheduling patch

The runtime guard is adopted from Omadesign 0.6.3 at
`c4813ef9a9628274778047e880a3aff6ea98d199`, applied to eframe 0.36.1.
`OMADESIGN_PROVENANCE.json` preserves its historical adoption receipt;
[`../EDITBAY_PROVENANCE.json`](../EDITBAY_PROVENANCE.json) records EditBay's final
source and manifest hashes, test layout and Clippy compatibility changes.

Visible redraws keep upstream `Poll`. Each window retains the time of its first
outstanding request; repeating it does not extend the allowance. Polling changes
to `Wait` only when every outstanding request is on Wayland and has remained
undelivered for at least 100 ms. Delivered, destroyed and directly painted hidden
windows leave the guard. A progressing window keeps polling. Explicit waits,
future deadlines and other backends retain their existing behavior.

Input, worker events and compositor callbacks wake the wait. The guard adds no
sleep and does not change document or motion timestamps. It bounds polling when
the compositor withholds frame callbacks; it is not a universal rendering or
latency optimization. `omadesign-wayland-wait.patch` retains the original runtime
patch. EditBay moves its four tests to the end of the implementation for Clippy.

The four tests now run as part of `cargo test --workspace --locked`. EditBay's
[native workspace receipts](../../docs/evidence/r1-workspace/README.md) qualify
actual input and recovery separately. The original Omadesign measurements are
historical evidence in the pinned reference checkout, not fresh EditBay results.

Upstream MIT/Apache and the copied Omadesign MIT notice are retained. The
normalized manifest follows EditBay's workspace resolver and removes a retired
Rust 1.98 Clippy lint; scoped upstream Clippy settings are restored.
