# EditBay

Build a native Rust media-production suite for Omarchy. Editing, motion, VFX,
color, audio, project recovery, and client/team workflows share a typed document
and command architecture.

## Start here

- `README.md`: entry points and working commands.
- `docs/IMPLEMENTATION_STATUS.md`: implemented capabilities and validation limits.
- `docs/ROADMAP.md`: stable implementation IDs and acceptance gates.
- `docs/ARCHITECTURE.md`: native module boundaries and concurrency contracts.
- `docs/OMADESIGN_REFERENCE.md`: welcome, recovery, project and cloud references.
- `docs/AI_MODELS.md`: inference candidates, offline matte workflow and model terms.

## Implementation

- Document/project/recovery source lives in `crates/editbay-core` and `crates/editbay-cli`.
  Native feasibility crates cover media/render/audio/AI and `editbay-lab` evidence
  tools. `crates/editbay-app` provides the native local workspace. `bin/editbay`
  launches it for no arguments/project paths and the CLI for named commands.
  Production media authoring and other roadmap modules remain open.
- Keep application code, production workers and project helper tools in Rust.
  Native C/C++ codec, GPU, color and inference libraries may use narrow adapters.
  Do not add a Python backend, inference subprocess, or parallel implementation.
- Use the Rust document/command path for UI, CLI, automation, undo and recovery.
  Reject stale jobs; keep expensive work off the UI thread; preserve user assets.
- Native UI should follow Omadesign's look and project/recovery behavior through
  independently versioned components. Do not modify Omadesign production services.
- Deliver working behavior with appropriate verification. Never add placeholder
  exporters, fake results, or enabled controls without implementations.
- Build/test/package Rust locally. Use `cargo test --workspace --locked`,
  `cargo fmt --all -- --check`, and relevant Clippy checks. Separate software
  evidence from native UI, physical hardware and live-service verification.
